use base64::{Engine as _, engine::general_purpose::STANDARD};
use pkcs8::der::{Decode, oid::ObjectIdentifier};
use pkcs8::{EncodePrivateKey, EncryptedPrivateKeyInfo, LineEnding, PrivateKeyInfo};
use rsa::{RsaPrivateKey, pkcs1::DecodeRsaPrivateKey};

use crate::AuthError;

#[derive(Debug)]
pub enum SnowflakeKeypairStatus<'a> {
    Clean,
    Warn3Des(&'a str),
}

pub const PEM_UNENCRYPTED_START: &str = "-----BEGIN PRIVATE KEY-----";
pub const PEM_UNENCRYPTED_END: &str = "-----END PRIVATE KEY-----";
pub const PEM_ENCRYPTED_START: &str = "-----BEGIN ENCRYPTED PRIVATE KEY-----";
pub const PEM_ENCRYPTED_END: &str = "-----END ENCRYPTED PRIVATE KEY-----";

const PEM_PKCS1_START: &str = "-----BEGIN RSA PRIVATE KEY-----";
const PEM_PKCS1_END: &str = "-----END RSA PRIVATE KEY-----";
const PKCS1_UNSUPPORTED_ERROR: &str = "\
Key is PKCS#1 (RSA private key). Snowflake requires PKCS#8 \
(-----BEGIN PRIVATE KEY----- or -----BEGIN ENCRYPTED PRIVATE KEY-----).\n\
\n\
Possible causes:\n\
• You ran 'openssl pkcs8 -inform pem -outform der ...' without -topk8\n\
• You wrapped a PKCS#1 DER with PKCS#8 headers\n\
• You base64-encoded PEM text and treated it as DER\n\
\n\
>   Generate a PKCS#8 key and update your user public key:\n\
https://docs.snowflake.com/en/user-guide/key-pair-auth\n\
While Snowflake recommends 3DES encryption, Fusion recommends using a modern algorithm such as AES-256:\n\
>   $ openssl genrsa 2048 | openssl pkcs8 -topk8 -v2 aes-256-cbc -inform PEM -out rsa_key.p8";

enum BodyKind {
    Pkcs8Unencrypted,
    Pkcs8Encrypted,
    Pkcs1Rsa, // legacy "RSA PRIVATE KEY", Snowflake only accepts pkcs8
    UnknownMalformed,
}

enum PemHeaderAndFooterState {
    Present,
    Missing,
    DoNotMatch,
}

#[inline]
fn detect_pem_state(pem: &str) -> PemHeaderAndFooterState {
    match (pem.contains("-----BEGIN "), pem.contains("-----END ")) {
        (true, true) => PemHeaderAndFooterState::Present,
        (false, false) => PemHeaderAndFooterState::Missing,
        _ => PemHeaderAndFooterState::DoNotMatch,
    }
}

#[inline]
fn has_single_valid_pem_pair(s: &str) -> bool {
    let has_begin_unencrypted = s.contains(PEM_UNENCRYPTED_START);
    let has_end_unencrypted = s.contains(PEM_UNENCRYPTED_END);
    let has_begin_encrypted = s.contains(PEM_ENCRYPTED_START);
    let has_end_encrypted = s.contains(PEM_ENCRYPTED_END);

    let has_only_unencrypted_pair =
        has_begin_unencrypted && has_end_unencrypted && !has_begin_encrypted && !has_end_encrypted;

    let has_only_encrypted_pair =
        has_begin_encrypted && has_end_encrypted && !has_begin_unencrypted && !has_end_unencrypted;

    has_only_unencrypted_pair || has_only_encrypted_pair
}

const OID_DES_EDE3_CBC: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.3.7");
const OID_P12_SHA_3KEY_3DES: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.12.1.3");

const THREE_DES_WARNING: &str = "------------------------------------------------------------------------ \n\
 Warning: You appear to be using a 3DES encrypted, headerless PEM body. We \n\
 encourage re-exporting your key using a modern algorithm such as AES-256: \n\
 >   $ openssl pkcs8 -topk8 -in rsa_key.pem -v2 aes256 -out rsa_key_aes.pem\n\
 \n\
 If you'd prefer to keep your current key and can supply your body PEM in a \n\
 multiline format, affix the customary header and footer:\n\
 >   -----BEGIN ENCRYPTED PRIVATE KEY-----\n\
 >   ...PEM BODY...\n\
 >   -----END ENCRYPTED PRIVATE KEY-----\n\
 \n\
 Note: Future releases may reject 3DES-encrypted keys entirely. \n\
 ------------------------------------------------------------------------";

/// Take the DER [1] payload of a key and recognize its format.
///
/// [1] https://en.wikipedia.org/wiki/X.690#DER_encoding
fn parse_der_key_type(der: &[u8]) -> (BodyKind, SnowflakeKeypairStatus<'static>) {
    if PrivateKeyInfo::from_der(der).is_ok() {
        (BodyKind::Pkcs8Unencrypted, SnowflakeKeypairStatus::Clean)
    } else if let Ok(enc) = EncryptedPrivateKeyInfo::from_der(der) {
        let alg = enc.encryption_algorithm;

        // TODO: Add a PBES1 3DES check?
        let is_pbes2: bool = if let Some(p) = alg.pbes2() {
            p.encryption.oid() == OID_DES_EDE3_CBC
        } else {
            Default::default()
        };

        // Some encoders use PKCS#12 3DES PBE as the top-level algorithm
        let is_p12_3des = alg.oid() == OID_P12_SHA_3KEY_3DES;

        let status = if is_p12_3des || is_pbes2 {
            SnowflakeKeypairStatus::Warn3Des(THREE_DES_WARNING)
        } else {
            SnowflakeKeypairStatus::Clean
        };

        (BodyKind::Pkcs8Encrypted, status)
    } else if RsaPrivateKey::from_pkcs1_der(der).is_ok() {
        (BodyKind::Pkcs1Rsa, SnowflakeKeypairStatus::Clean)
    } else {
        (BodyKind::UnknownMalformed, SnowflakeKeypairStatus::Clean)
    }
}

/// Wrap a DER (base64) using chosen header.
/// We decode+re-encode to normalize whitespace and line-length.
///
/// [2] https://en.wikipedia.org/wiki/Privacy-Enhanced_Mail
fn wrap_der_as_pem(der: &[u8], header: &str, footer: &str) -> String {
    let b64 = STANDARD.encode(der);
    let mut out = String::with_capacity(header.len() + footer.len() + b64.len() + 128);
    out.push_str(header);
    out.push('\n');
    for chunk in b64.as_bytes().chunks(64) {
        // base64 is ASCII
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(footer);
    out
}

#[inline]
fn looks_like_pem_text_after_decode(bytes: &[u8]) -> bool {
    // Tolerate BOM: https://en.wikipedia.org/wiki/Byte_order_mark
    let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };

    if !bytes
        .iter()
        .all(|&b| b.is_ascii() && (b.is_ascii_graphic() || b.is_ascii_whitespace()))
    {
        return false;
    }

    // Only look in the first 256 bytes for the PEM preamble. Possible characters ahead
    // of this include BOM, untrimmed whitespace that we can tolerate, etc.
    let head = &bytes[..bytes.len().min(256)];
    const NEEDLE: &[u8] = b"-----BEGIN ";
    head.windows(NEEDLE.len()).any(|w| w == NEEDLE)
}

fn has_pkcs1_pem_pair(s: &str) -> bool {
    s.contains(PEM_PKCS1_START) && s.contains(PEM_PKCS1_END)
}

/// Main entry:
/// A base64 DER is the body of a PEM
/// - If BOTH headers present -> return as-is.
/// - If NEITHER header present -> classify and wrap with the matching header.
///   - PKCS#8 unencrypted -> `BEGIN PRIVATE KEY`
///   - PKCS#8 encrypted   -> `BEGIN ENCRYPTED PRIVATE KEY`
///   - PKCS#1 RSA         -> error (ask caller to convert to PKCS#8)
/// - If only one header present -> error.
pub fn normalize_key(input: &str) -> Result<(String, SnowflakeKeypairStatus<'_>), AuthError> {
    let trimmed_input = input.trim();
    match detect_pem_state(trimmed_input) {
        PemHeaderAndFooterState::Present => {
            if has_pkcs1_pem_pair(trimmed_input) {
                return Err(AuthError::Config(PKCS1_UNSUPPORTED_ERROR.to_string()));
            }
            has_single_valid_pem_pair(trimmed_input)
                .then(|| (input.to_string(), SnowflakeKeypairStatus::Clean))
                .ok_or_else(|| {
                    AuthError::config("malformed key: missing or mismatched BEGIN/END header pair")
                })
        }
        // headerless input - may be DER with hidden headers or PEM body
        PemHeaderAndFooterState::Missing => {
            // strip all internal whitespace so strict base64 decode succeeds.
            let cleaned: String = trimmed_input
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();

            let decoded = STANDARD
                .decode(cleaned.as_bytes())
                .map_err(|e| AuthError::config(format!("invalid base64 in key body: {e}")))?;

            // case a: the decoded bytes are actually pem text that was base64-encoded.
            if looks_like_pem_text_after_decode(&decoded) {
                let pem = String::from_utf8(decoded).map_err(|_| {
                    AuthError::config("decoded base64 looked like PEM but was not UTF-8")
                })?;
                return Ok((pem.trim().to_string(), SnowflakeKeypairStatus::Clean));
            }

            // case b: treat as der and classify.
            let (body_kind, status) = parse_der_key_type(&decoded);
            match body_kind {
                BodyKind::Pkcs8Unencrypted => Ok((
                    wrap_der_as_pem(&decoded, PEM_UNENCRYPTED_START, PEM_UNENCRYPTED_END),
                    status,
                )),
                BodyKind::Pkcs8Encrypted => Ok((
                    wrap_der_as_pem(&decoded, PEM_ENCRYPTED_START, PEM_ENCRYPTED_END),
                    status,
                )),
                BodyKind::Pkcs1Rsa => Err(AuthError::config(PKCS1_UNSUPPORTED_ERROR.to_string())),
                BodyKind::UnknownMalformed => Err(AuthError::config("key body is not PKCS#8 DER.")),
            }
        }
        PemHeaderAndFooterState::DoNotMatch => Err(AuthError::config(
            "malformed key: BEGIN/END header mismatch",
        )),
    }
}

/// Re-encode a decrypted key as unencrypted PKCS#8 PEM, e.g. for a caller
/// that needs to hand PEM text to a library expecting that input shape
/// rather than a parsed key.
pub fn to_pkcs8_pem(key: &RsaPrivateKey) -> Result<String, AuthError> {
    key.to_pkcs8_pem(LineEnding::LF)
        .map(|pem| pem.to_string())
        .map_err(|e| AuthError::config(format!("could not re-encode private key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_test_primitives::assert_contains;
    use pkcs8::DecodePrivateKey;
    use rsa::{RsaPrivateKey, pkcs1::EncodeRsaPrivateKey, rand_core::OsRng};

    fn gen_rsa() -> RsaPrivateKey {
        RsaPrivateKey::new(&mut OsRng, 2048).unwrap()
    }

    fn to_pkcs8_unenc_pem(rsa: &RsaPrivateKey) -> String {
        rsa.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()
    }

    fn to_pkcs8_unenc_der_b64(rsa: &RsaPrivateKey) -> String {
        let der = rsa.to_pkcs8_der().unwrap();
        STANDARD.encode(der.as_bytes())
    }

    fn to_base64_of_pem_text(pem: &str) -> String {
        STANDARD.encode(pem.as_bytes())
    }

    fn to_pkcs1_der_b64(rsa: &RsaPrivateKey) -> String {
        let der = rsa.to_pkcs1_der().unwrap();
        STANDARD.encode(der.as_bytes())
    }

    #[test]
    fn pkcs8_unencrypted_pem_passthrough() {
        let rsa = gen_rsa();
        let pem = to_pkcs8_unenc_pem(&rsa);
        let norm = normalize_key(&pem).unwrap().0;
        assert_eq!(norm, pem);
    }

    #[test]
    fn pkcs8_unencrypted_der_b64_wraps() {
        let rsa = gen_rsa();
        let b64 = to_pkcs8_unenc_der_b64(&rsa);
        let norm = normalize_key(&b64).unwrap().0;
        assert!(norm.starts_with(PEM_UNENCRYPTED_START));
    }

    #[test]
    fn base64_of_pem_text_unwraps() {
        let rsa = gen_rsa();
        let pem = to_pkcs8_unenc_pem(&rsa);
        let b64_of_pem = to_base64_of_pem_text(&pem);
        let norm = normalize_key(&b64_of_pem).unwrap().0;
        assert_eq!(norm, pem.trim());
    }

    #[test]
    fn pkcs1_der_b64_errors() {
        let rsa = gen_rsa();
        let b64_pkcs1 = to_pkcs1_der_b64(&rsa);
        let err = normalize_key(&b64_pkcs1).unwrap_err();
        let msg = format!("{err:?}");
        assert_contains!(msg, "PKCS#1");
    }

    #[test]
    fn header_mismatch_errors() {
        let pem = format!("{PEM_UNENCRYPTED_START}\nMII...\n{PEM_ENCRYPTED_END}");
        let err = normalize_key(&pem).unwrap_err();
        assert!(format!("{err:?}").contains("malformed key"));
    }

    #[test]
    fn unknown_malformed_errors() {
        let bad_b64 = STANDARD.encode(b"not-a-real-key");
        let err = normalize_key(&bad_b64).unwrap_err();
        assert!(format!("{err:?}").contains("key body is not PKCS#8"));
    }
    #[test]
    fn pem_encrypted_passthrough() {
        // Take a valid unencrypted PKCS#8 PEM and relabel headers to ENCRYPTED.
        // We only test strict header pairing passthrough here; content is not re-validated.
        let rsa = gen_rsa();
        let pem_unenc = to_pkcs8_unenc_pem(&rsa);
        let pem_enc = pem_unenc
            .replace(PEM_UNENCRYPTED_START, PEM_ENCRYPTED_START)
            .replace(PEM_UNENCRYPTED_END, PEM_ENCRYPTED_END);

        let norm = normalize_key(&pem_enc).unwrap().0;
        assert_eq!(norm, pem_enc, "encrypted PEM should pass through unchanged");
    }

    #[test]
    fn pem_pkcs1_rejected() {
        // PKCS#1 PEM should be rejected in the PEM branch (headers don't match pkcs8).
        let rsa = gen_rsa();
        let pkcs1_pem = rsa.to_pkcs1_pem(LineEnding::LF).unwrap().to_string();
        let err = normalize_key(&pkcs1_pem).unwrap_err();
        assert!(
            format!("{err:?}").contains("PKCS#1"),
            "PKCS#1 PEM must not be accepted as PKCS#8 PEM"
        )
    }

    #[test]
    fn begin_without_end_errors() {
        let pem = format!("{PEM_UNENCRYPTED_START}\nMII...\n"); // missing END line
        let err = normalize_key(&pem).unwrap_err();
        assert!(format!("{err:?}").contains("BEGIN/END header mismatch"));
    }

    #[test]
    fn both_pairs_present_errors() {
        // Both unencrypted and encrypted pairs present should be rejected (strict pairing).
        let pem = format!(
            "{PEM_UNENCRYPTED_START}\nMII...\n{PEM_UNENCRYPTED_END}\n{PEM_ENCRYPTED_START}\n..."
        );
        let err = normalize_key(&pem).unwrap_err();
        assert!(format!("{err:?}").contains("missing or mismatched BEGIN/END"));
    }

    #[test]
    fn stray_other_header_errors() {
        // Valid unencrypted pair + stray encrypted BEGIN should be rejected.
        let pem = format!(
            "{PEM_UNENCRYPTED_START}\nMII...\n{PEM_UNENCRYPTED_END}\n{PEM_ENCRYPTED_START}\n"
        );
        let err = normalize_key(&pem).unwrap_err();
        assert!(format!("{err:?}").contains("missing or mismatched BEGIN/END"));
    }

    #[test]
    fn headerless_b64_with_whitespace_wraps() {
        // Ensure we strip interior whitespace before decoding and wrap to PEM.
        let rsa = gen_rsa();
        let b64 = to_pkcs8_unenc_der_b64(&rsa);
        // Insert arbitrary whitespace
        let noisy = format!(
            "  {}\n \t {}\r\n",
            &b64[..b64.len() / 2],
            &b64[b64.len() / 2..]
        );
        let norm = normalize_key(&noisy).unwrap().0;
        assert!(norm.starts_with(PEM_UNENCRYPTED_START));
        assert!(
            norm.ends_with(PEM_UNENCRYPTED_END),
            "END line should be last line"
        );
    }

    #[test]
    fn line_wrapping_is_64_chars() {
        // Verify emitted PEM body lines are wrapped at 64 chars (except the last).
        let rsa = gen_rsa();
        let b64 = to_pkcs8_unenc_der_b64(&rsa);
        let norm = normalize_key(&b64).unwrap().0;

        // Extract body between headers
        let body = norm
            .lines()
            .skip(1) // after BEGIN
            .take_while(|line| !line.starts_with(PEM_UNENCRYPTED_END))
            .collect::<Vec<_>>();
        assert!(!body.is_empty());
        for (i, line) in body.iter().enumerate() {
            if i + 1 < body.len() {
                assert_eq!(line.len(), 64, "non-final body lines must be 64 chars");
            } else {
                assert!(line.len() <= 64, "final body line must be <= 64 chars");
            }
        }
    }

    #[test]
    fn empty_input_errors() {
        let err = normalize_key("").unwrap_err();
        assert!(format!("{err:?}").contains("key body is not PKCS#8"));
    }

    #[test]
    fn base64_of_encrypted_pem_text_unwraps() {
        // Exercise the base64(PEM text) path with ENCRYPTED headers.
        let rsa = gen_rsa();
        let pem_unenc = to_pkcs8_unenc_pem(&rsa);
        let pem_enc = pem_unenc
            .replace(PEM_UNENCRYPTED_START, PEM_ENCRYPTED_START)
            .replace(PEM_UNENCRYPTED_END, PEM_ENCRYPTED_END);
        let b64_of_pem = to_base64_of_pem_text(&pem_enc);

        let norm = normalize_key(&b64_of_pem).unwrap().0;
        assert_eq!(
            norm,
            pem_enc.trim(),
            "should unwrap to original ENCRYPTED PEM"
        );
    }

    #[test]
    fn open_ssh_pem_rejected() {
        // Non-PKCS#8 PEM header should be rejected in PEM-branch.
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA...\n-----END OPENSSH PRIVATE KEY-----";
        let err = normalize_key(pem).unwrap_err();
        assert!(format!("{err:?}").contains("malformed key"));
    }

    #[test]
    fn headerless_pbes2_3des_body_is_wrapped_as_encrypted_pem() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        // Minimal EncryptedPrivateKeyInfo (PBES2 + 3DES-CBC)
        //   Command used to generate:
        //
        //   hex='303d303806092a864886f70d01050d302b301306092a864886f70d01050c3006040100020101301406082a864886f70d03070408aaaaaaaaaaaaaaaa040100'
        //   printf '%s' "$hex" | xxd -r -p > minimal.der
        //   openssl asn1parse -inform DER -in minimal.der -i
        //   openssl base64 -A -in minimal.der > body.b64
        //   { echo '-----BEGIN ENCRYPTED PRIVATE KEY-----'; fold -w64 body.b64; echo '-----END ENCRYPTED PRIVATE KEY-----'; } > minimal.pem
        //   diff <(openssl base64 -d -A < body.b64) minimal.der && echo 'OK'
        let der: Vec<u8> = vec![
            0x30, 0x3D, // EncryptedPrivateKeyInfo SEQ (61)
            0x30, 0x38, // AlgorithmIdentifier SEQ (56)
            0x06, 0x09, //   OID PBES2 (1.2.840.113549.1.5.13)
            0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05, 0x0D, 0x30,
            0x2B, //   PBES2-params SEQ (43)
            0x30, 0x13, //     keyDerivationFunc: AlgorithmIdentifier (19)
            0x06, 0x09, //       OID PBKDF2 (1.2.840.113549.1.5.12)
            0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05, 0x0C, 0x30,
            0x06, //       PBKDF2-params SEQ (6)
            0x04, 0x01, 0x00, //         salt OCTET STRING (1B)
            0x02, 0x01, 0x01, //         iter INTEGER (1)
            0x30, 0x14, //     encryptionScheme: AlgorithmIdentifier (20)
            0x06, 0x08, //       OID des-ede3-cbc (1.2.840.113549.3.7)
            0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x03, 0x07, 0x04,
            0x08, //       IV OCTET STRING (8B)
            0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0x04, 0x01,
            0x00, // encryptedData OCTET STRING (1B)
        ];

        let body_b64 = STANDARD.encode(&der);

        let normalized = normalize_key(&body_b64).expect("normalize_key failed").0;

        assert!(
            normalized.starts_with(PEM_ENCRYPTED_START),
            "expected BEGIN ENCRYPTED PRIVATE KEY header"
        );
        assert!(
            normalized.ends_with(PEM_ENCRYPTED_END),
            "expected END ENCRYPTED PRIVATE KEY footer"
        );

        let re_body = normalized
            .lines()
            .skip(1)
            .take_while(|l| !l.starts_with(PEM_ENCRYPTED_END))
            .collect::<String>();

        let der_roundtrip = STANDARD
            .decode(re_body.as_bytes())
            .expect("emitted body not base64");
        assert_eq!(der_roundtrip, der, "PEM rewrap must be byte-preserving");
    }

    #[test]
    fn to_pkcs8_pem_roundtrips_a_key() {
        let rsa = gen_rsa();
        let pem = to_pkcs8_pem(&rsa).unwrap();
        assert!(pem.starts_with(PEM_UNENCRYPTED_START));
        assert!(pem.trim_end().ends_with(PEM_UNENCRYPTED_END));

        let decoded = RsaPrivateKey::from_pkcs8_pem(&pem).unwrap();
        assert_eq!(
            decoded, rsa,
            "re-encoded PEM must decode back to the same key"
        );
    }
}
