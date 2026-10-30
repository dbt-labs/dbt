#![deny(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr;

use tempfile::NamedTempFile;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError,
};
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
use windows_sys::Win32::Security::{
    ACL, ACL_REVISION, AddAccessAllowedAceEx, InitializeAcl, InitializeSecurityDescriptor,
    SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, SetSecurityDescriptorControl,
    SetSecurityDescriptorDacl, TOKEN_QUERY,
};
use windows_sys::Win32::Security::{
    CopySid, GetLengthSid, GetTokenInformation, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ALL_ACCESS,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_ALWAYS, OPEN_EXISTING,
    READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::SystemServices::SECURITY_DESCRIPTOR_REVISION;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const TRUE: i32 = 1;
const FALSE: i32 = 0;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const FILE_ACCESS: u32 = FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | WRITE_DAC;
const FILE_SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

/// Open a file for writing, replacing its contents after its DACL is protected.
pub fn open_owner_only(path: &Path) -> io::Result<File> {
    let mut descriptor = ProtectedDescriptor::new(0)?;
    let (file, created) = create_file(path, OPEN_ALWAYS, &mut descriptor)?;

    if let Err(error) = file.set_len(0) {
        drop(file);
        if created {
            let _ = remove_file(path);
        }
        return Err(error);
    }

    Ok(file)
}

/// Create a named temporary file whose DACL is protected before it is returned.
pub fn owner_only_tempfile_in(parent: &Path) -> io::Result<NamedTempFile> {
    tempfile::Builder::new().make_in(parent, |path| {
        let mut descriptor = ProtectedDescriptor::new(0)?;
        let (file, _) = create_file(path, CREATE_NEW, &mut descriptor)?;
        Ok(file)
    })
}

/// Create or repair the leaf directory with an inheritable owner-only DACL.
pub fn ensure_owner_only_dir(path: &Path) -> io::Result<()> {
    let mut descriptor = ProtectedDescriptor::new(
        windows_sys::Win32::Security::OBJECT_INHERIT_ACE
            | windows_sys::Win32::Security::CONTAINER_INHERIT_ACE,
    )?;
    let path_w = to_wide(path)?;
    let attributes = descriptor.security_attributes();
    // SAFETY: path_w is NUL-terminated and attributes points to the live
    // descriptor for the duration of the call.
    let created = unsafe { CreateDirectoryW(path_w.as_ptr(), &attributes) != FALSE };
    let creation_error = if created {
        None
    } else {
        // SAFETY: GetLastError reads the error from the immediately preceding
        // CreateDirectoryW call on this thread.
        Some(unsafe { GetLastError() })
    };

    if !created && creation_error != Some(ERROR_ALREADY_EXISTS) {
        return Err(win32_error(creation_error.unwrap_or_default()));
    }

    // SAFETY: path_w is NUL-terminated; the access, sharing, and flags are
    // valid for opening the directory handle.
    let handle = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    let open_error = if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        // SAFETY: GetLastError reads the error from the immediately preceding
        // CreateFileW call on this thread.
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    if let Some(error) = open_error {
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(win32_error(error));
    }

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: handle is a valid directory handle returned by CreateFileW and
    // information is a writable output buffer of the documented type.
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == FALSE {
        let error = last_error();
        // SAFETY: handle is still owned by this function and is closed before
        // any path cleanup.
        unsafe {
            CloseHandle(handle);
        }
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(error);
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        // SAFETY: handle is still owned by this function and is closed before
        // returning the reparse-point error.
        unsafe {
            CloseHandle(handle);
        }
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is a reparse point",
        ));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        // SAFETY: handle is still owned by this function and is closed before
        // returning the type error.
        unsafe {
            CloseHandle(handle);
        }
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path is not a directory",
        ));
    }

    let security_result = set_protected_dacl(handle, &descriptor);
    // SAFETY: handle is still owned by this function and is closed exactly
    // once after the security operation completes.
    let close_result = unsafe { CloseHandle(handle) };
    if let Err(error) = security_result {
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(error);
    }
    if close_result == FALSE {
        // SAFETY: GetLastError reads the error from the immediately preceding
        // CloseHandle call on this thread.
        let error = unsafe { GetLastError() };
        if created {
            let _ = std::fs::remove_dir(path);
        }
        return Err(win32_error(error));
    }

    Ok(())
}

fn create_file(
    path: &Path,
    disposition: u32,
    descriptor: &mut ProtectedDescriptor,
) -> io::Result<(File, bool)> {
    let path_w = to_wide(path)?;
    let attributes = descriptor.security_attributes();
    // SAFETY: path_w is NUL-terminated; attributes points to the live
    // descriptor for the duration of the call.
    let handle = unsafe {
        SetLastError(0);
        CreateFileW(
            path_w.as_ptr(),
            FILE_ACCESS,
            FILE_SHARE,
            &attributes,
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    // SAFETY: GetLastError reads the error or status from the immediately
    // preceding CreateFileW call on this thread.
    let creation_error = unsafe { GetLastError() };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(win32_error(creation_error));
    }

    let created = creation_error != ERROR_ALREADY_EXISTS;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle was returned by CreateFileW and remains owned by this
    // function until it is converted into a File or closed on an error path.
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == FALSE {
        let error = last_error();
        cleanup_file_handle(handle, &path_w, created);
        return Err(error);
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        close_file_handle(handle);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is a reparse point",
        ));
    }

    if let Err(error) = set_protected_dacl(handle, descriptor) {
        cleanup_file_handle(handle, &path_w, created);
        return Err(error);
    }

    // SAFETY: CreateFileW returned a valid, uniquely owned handle and no other
    // owner of the handle exists after this conversion.
    let file = unsafe { File::from_raw_handle(handle as RawHandle) };
    Ok((file, created))
}

fn cleanup_file_handle(handle: HANDLE, path_w: &[u16], created: bool) {
    close_file_handle(handle);
    if created {
        let _ = delete_file_wide(path_w);
    }
}

fn close_file_handle(handle: HANDLE) {
    // SAFETY: the handle came from a successful CreateFileW call and has not
    // been converted into a File, so this closes its sole ownership here.
    unsafe {
        CloseHandle(handle);
    }
}

fn set_protected_dacl(handle: HANDLE, descriptor: &ProtectedDescriptor) -> io::Result<()> {
    // SAFETY: handle is owned by the caller for the duration of this call, and
    // descriptor.dacl() points to the live ACL backing the security descriptor.
    let result = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            windows_sys::Win32::Security::DACL_SECURITY_INFORMATION
                | windows_sys::Win32::Security::PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            descriptor.dacl(),
            ptr::null(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(win32_error(result))
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    let path_w = to_wide(path)?;
    delete_file_wide(&path_w)
}

fn delete_file_wide(path: &[u16]) -> io::Result<()> {
    // SAFETY: callers pass a NUL-terminated UTF-16 path buffer.
    if unsafe { windows_sys::Win32::Storage::FileSystem::DeleteFileW(path.as_ptr()) } == FALSE {
        Err(last_error())
    } else {
        Ok(())
    }
}

fn to_wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut path_w: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path_w.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an embedded NUL",
        ));
    }
    path_w.push(0);
    Ok(path_w)
}

fn win32_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError reads the calling thread's Win32 error value.
    win32_error(unsafe { GetLastError() })
}

struct ProtectedDescriptor {
    descriptor: SECURITY_DESCRIPTOR,
    acl: Vec<u32>,
    _sid: Vec<u32>,
}

impl ProtectedDescriptor {
    fn new(ace_flags: u32) -> io::Result<Self> {
        let sid = current_user_sid()?;
        let acl_size = size_of::<ACL>()
            .checked_add(size_of::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>())
            .and_then(|size| size.checked_add(sid.len() * size_of::<u32>()))
            .and_then(|size| size.checked_sub(size_of::<u32>()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ACL size overflow"))?;
        let acl_words = acl_size.div_ceil(size_of::<u32>());
        let acl_bytes = acl_words
            .checked_mul(size_of::<u32>())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ACL size overflow"))?;
        if acl_bytes > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACL is too large",
            ));
        }

        let mut acl = vec![0_u32; acl_words];
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        // SAFETY: descriptor points to writable storage of the documented
        // SECURITY_DESCRIPTOR type.
        if unsafe {
            InitializeSecurityDescriptor(
                &mut descriptor as *mut SECURITY_DESCRIPTOR as *mut c_void,
                SECURITY_DESCRIPTOR_REVISION,
            )
        } == FALSE
        {
            return Err(last_error());
        }
        // SAFETY: acl is an aligned writable buffer sized for the requested
        // ACL, and ACL_REVISION is the supported revision.
        if unsafe { InitializeAcl(acl.as_mut_ptr() as *mut ACL, acl_bytes as u32, ACL_REVISION) }
            == FALSE
        {
            return Err(last_error());
        }
        // SAFETY: acl and sid remain alive and valid for the duration of the
        // ACE construction call.
        if unsafe {
            AddAccessAllowedAceEx(
                acl.as_mut_ptr() as *mut ACL,
                ACL_REVISION,
                ace_flags,
                FILE_ALL_ACCESS,
                sid.as_ptr() as *mut c_void,
            )
        } == FALSE
        {
            return Err(last_error());
        }
        // SAFETY: descriptor and acl remain alive and valid for the duration of
        // the security-descriptor update.
        if unsafe {
            SetSecurityDescriptorDacl(
                &mut descriptor as *mut SECURITY_DESCRIPTOR as *mut c_void,
                TRUE,
                acl.as_ptr() as *const ACL,
                FALSE,
            )
        } == FALSE
        {
            return Err(last_error());
        }
        // SAFETY: descriptor is initialized and remains valid for the duration
        // of the control update.
        if unsafe {
            SetSecurityDescriptorControl(
                &mut descriptor as *mut SECURITY_DESCRIPTOR as *mut c_void,
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            )
        } == FALSE
        {
            return Err(last_error());
        }

        Ok(Self {
            descriptor,
            acl,
            _sid: sid,
        })
    }

    fn dacl(&self) -> *const ACL {
        self.acl.as_ptr() as *const ACL
    }

    fn security_attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: &mut self.descriptor as *mut SECURITY_DESCRIPTOR as *mut c_void,
            bInheritHandle: FALSE,
        }
    }
}

fn current_user_sid() -> io::Result<Vec<u32>> {
    let mut token = ptr::null_mut();
    // SAFETY: token is a writable output pointer and GetCurrentProcess returns
    // the pseudo-handle for the current process.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == FALSE {
        return Err(last_error());
    }
    let token = OwnedHandle::new(token)?;

    let mut needed = 0_u32;
    // SAFETY: this is the documented sizing query with a null output buffer.
    let _ = unsafe { GetTokenInformation(token.raw(), TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(last_error());
    }
    let mut token_user_buffer = vec![0_u8; needed as usize];
    // SAFETY: token_user_buffer is writable storage of the size returned by
    // the preceding sizing query.
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            token_user_buffer.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        )
    } == FALSE
    {
        return Err(last_error());
    }
    if token_user_buffer.len() < size_of::<TOKEN_USER>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token user information is truncated",
        ));
    }

    // SAFETY: GetTokenInformation filled at least TOKEN_USER bytes. The byte
    // buffer has no alignment guarantee, so read_unaligned is required.
    let token_user =
        unsafe { ptr::read_unaligned(token_user_buffer.as_ptr() as *const TOKEN_USER) };
    let token_sid = token_user.User.Sid;
    if token_sid.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "token user information has no SID",
        ));
    }
    // SAFETY: token_sid was returned inside validated TOKEN_USER data.
    let sid_len = unsafe { GetLengthSid(token_sid) };
    if sid_len == 0 {
        return Err(last_error());
    }
    let sid_words = (sid_len as usize).div_ceil(size_of::<u32>());
    let mut sid = vec![0_u32; sid_words];
    // SAFETY: sid is writable storage large enough for sid_len bytes, and
    // token_sid points to the validated token SID.
    if unsafe { CopySid(sid_len, sid.as_mut_ptr() as *mut c_void, token_sid) } == FALSE {
        return Err(last_error());
    }
    Ok(sid)
}

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(last_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the token handle and drops it exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::windows::fs::{OpenOptionsExt, symlink_file};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_PRIVILEGE_NOT_HELD, HLOCAL, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation, AllocateAndInitializeSid,
        CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, FreeSid, GetAce,
        GetAclInformation, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, INHERITED_ACE,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        SECURITY_CREATOR_SID_AUTHORITY,
    };
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE};
    use windows_sys::Win32::System::SystemServices::SECURITY_CREATOR_OWNER_RIGHTS_RID;

    #[test]
    fn creates_a_protected_owner_only_file_before_writing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("secret");
        let mut file = open_owner_only(&path).unwrap();
        file.write_all(b"secret").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"secret");
        assert_acl(&path, 0, false, true);
    }

    #[test]
    fn repairs_existing_file_before_truncation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("secret");
        std::fs::write(&path, b"old secret").unwrap();
        assert_dacl_protection(&path, false);

        let file = open_owner_only(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        assert_acl(&path, 0, false, true);
    }

    #[test]
    fn protects_a_temp_file_and_keeps_protection_after_persist() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("secret");
        std::fs::write(&target, b"old secret").unwrap();
        assert_dacl_protection(&target, false);
        let mut file = owner_only_tempfile_in(root.path()).unwrap();
        assert_acl(file.path(), 0, false, true);
        file.write_all(b"secret").unwrap();
        file.persist(&target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"secret");
        assert_acl(&target, 0, false, true);
    }

    #[test]
    fn protects_a_directory_and_inherits_to_children() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        ensure_owner_only_dir(&private).unwrap();
        assert_acl(
            &private,
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
            false,
            true,
        );

        let child = private.join("child");
        std::fs::File::create(&child).unwrap();
        assert_acl(&child, INHERITED_ACE, true, false);
    }

    #[test]
    fn rejects_a_directory_as_a_file_without_truncating_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("directory");
        ensure_owner_only_dir(&path).unwrap();

        assert!(open_owner_only(&path).is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn rejects_a_file_as_a_directory_without_changing_it() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        std::fs::write(&path, b"contents").unwrap();

        let error = ensure_owner_only_dir(&path).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
        assert_eq!(std::fs::read(&path).unwrap(), b"contents");
    }

    #[test]
    fn does_not_truncate_when_dacl_cannot_be_repaired() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("secret");
        std::fs::write(&path, b"old secret").unwrap();
        set_owner_rights_dacl(&path);
        assert_owner_rights_dacl(&path);
        OpenOptions::new().read(true).open(&path).unwrap();
        let write_dac_error = match OpenOptions::new().access_mode(WRITE_DAC).open(&path) {
            Ok(_) => panic!("OWNER RIGHTS unexpectedly granted WRITE_DAC"),
            Err(error) => error,
        };
        assert_eq!(write_dac_error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            write_dac_error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );

        // The helper requests WRITE_DAC in CreateFileW, so this denial occurs
        // during open rather than in SetSecurityInfo. This test covers the
        // fail-closed pre-truncation boundary, not SetSecurityInfo failures.
        let error = match open_owner_only(&path) {
            Ok(_) => panic!("open_owner_only unexpectedly repaired the DACL"),
            Err(error) => error,
        };
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
        assert_eq!(std::fs::read(&path).unwrap(), b"old secret");
        assert_owner_rights_dacl(&path);
    }

    #[test]
    fn rejects_symlink_without_touching_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("link");
        std::fs::write(&target, b"old secret").unwrap();
        assert_dacl_protection(&target, false);
        if !create_test_symlink(&target, &link) {
            return;
        }

        let error = open_owner_only(&link).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&target).unwrap(), b"old secret");
        assert_dacl_protection(&target, false);
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn rejects_dangling_symlink_without_creating_target() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let link = root.path().join("link");
        if !create_test_symlink(&target, &link) {
            return;
        }

        let error = open_owner_only(&link).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!target.exists());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    fn create_test_symlink(target: &Path, link: &Path) -> bool {
        match symlink_file(target, link) {
            Ok(()) => true,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD as i32)
                    && std::env::var_os("CI").is_none() =>
            {
                eprintln!(
                    "skipping symlink test because SeCreateSymbolicLinkPrivilege is unavailable outside CI"
                );
                false
            }
            Err(error) => panic!("failed to create test symlink: {error}"),
        }
    }

    fn assert_dacl_protection(path: &Path, expected: bool) {
        let path_w = to_wide(path).unwrap();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: all output pointers refer to writable local storage and the
        // path buffer is NUL-terminated.
        let status = unsafe {
            GetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0, "GetNamedSecurityInfoW failed: {status}");
        assert!(!descriptor.is_null());

        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: descriptor was allocated and initialized by
        // GetNamedSecurityInfoW above.
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_eq!(control & SE_DACL_PROTECTED != 0, expected);

        // SAFETY: descriptor was allocated by GetNamedSecurityInfoW and must
        // be released with LocalFree.
        unsafe {
            LocalFree(descriptor as HLOCAL);
        }
    }

    fn set_owner_rights_dacl(path: &Path) {
        let mut owner_rights_sid = ptr::null_mut();
        // SAFETY: owner_rights_sid is a writable output pointer and the SID
        // authority and subauthority are fixed valid inputs.
        assert_ne!(
            unsafe {
                AllocateAndInitializeSid(
                    &SECURITY_CREATOR_SID_AUTHORITY,
                    1,
                    SECURITY_CREATOR_OWNER_RIGHTS_RID as u32,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    &mut owner_rights_sid,
                )
            },
            0
        );

        // SAFETY: owner_rights_sid was allocated successfully above.
        let sid_len = unsafe { GetLengthSid(owner_rights_sid) } as usize;
        let acl_bytes =
            size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() + sid_len - size_of::<u32>();
        let mut acl = vec![0_u32; acl_bytes.div_ceil(size_of::<u32>())];
        // SAFETY: acl is aligned writable storage sized for the ACL.
        assert_ne!(
            unsafe {
                InitializeAcl(
                    acl.as_mut_ptr() as *mut ACL,
                    (acl.len() * size_of::<u32>()) as u32,
                    ACL_REVISION,
                )
            },
            0
        );
        // SAFETY: acl and owner_rights_sid remain valid for this ACE append.
        assert_ne!(
            unsafe {
                AddAccessAllowedAceEx(
                    acl.as_mut_ptr() as *mut ACL,
                    ACL_REVISION,
                    0,
                    FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE,
                    owner_rights_sid,
                )
            },
            0
        );

        let path_w = to_wide(path).unwrap();
        // SAFETY: path_w is NUL-terminated and acl remains alive for the
        // duration of this security update.
        let status = unsafe {
            SetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl.as_ptr() as *const ACL,
                ptr::null(),
            )
        };
        // SAFETY: owner_rights_sid was allocated by AllocateAndInitializeSid
        // and is released exactly once here.
        unsafe {
            FreeSid(owner_rights_sid);
        }
        assert_eq!(status, 0, "SetNamedSecurityInfoW failed: {status}");
    }

    fn assert_owner_rights_dacl(path: &Path) {
        let path_w = to_wide(path).unwrap();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: all output pointers refer to writable local storage and the
        // path buffer is NUL-terminated.
        let status = unsafe {
            GetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0, "GetNamedSecurityInfoW failed: {status}");
        assert!(!descriptor.is_null());

        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: descriptor was allocated and initialized by
        // GetNamedSecurityInfoW above.
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_ne!(control & SE_DACL_PROTECTED, 0);

        let mut dacl_present = 0_i32;
        let mut dacl_defaulted = 0_i32;
        // SAFETY: descriptor and dacl point to storage returned by the
        // successful GetNamedSecurityInfoW call.
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(
                    descriptor,
                    &mut dacl_present,
                    &mut dacl,
                    &mut dacl_defaulted,
                )
            },
            0
        );
        assert_ne!(dacl_present, 0);
        assert!(!dacl.is_null());

        let mut size = ACL_SIZE_INFORMATION::default();
        // SAFETY: dacl points to the descriptor's valid DACL and size is a
        // writable output buffer of the documented type.
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    &mut size as *mut ACL_SIZE_INFORMATION as *mut c_void,
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        assert_eq!(size.AceCount, 1);

        let mut ace = ptr::null_mut();
        // SAFETY: dacl is valid and ace is a writable output pointer.
        assert_ne!(unsafe { GetAce(dacl, 0, &mut ace) }, 0);
        // SAFETY: GetAce returned a valid ACCESS_ALLOWED_ACE pointer for ACE 0.
        let ace = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(ace.Mask, FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE);

        let mut owner_rights_sid = ptr::null_mut();
        // SAFETY: owner_rights_sid is a writable output pointer and the SID
        // authority and subauthority are fixed valid inputs.
        assert_ne!(
            unsafe {
                AllocateAndInitializeSid(
                    &SECURITY_CREATOR_SID_AUTHORITY,
                    1,
                    SECURITY_CREATOR_OWNER_RIGHTS_RID as u32,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    &mut owner_rights_sid,
                )
            },
            0
        );
        let ace_sid = &ace.SidStart as *const u32 as *mut c_void;
        // SAFETY: ace_sid and owner_rights_sid point to valid SID values.
        assert_ne!(unsafe { EqualSid(ace_sid, owner_rights_sid) }, 0);
        // SAFETY: both the allocated SID and security descriptor are released
        // exactly once here.
        unsafe {
            FreeSid(owner_rights_sid);
            LocalFree(descriptor as HLOCAL);
        }
    }

    fn assert_acl(path: &Path, expected_flags: u32, inherited: bool, protected: bool) {
        let path_w = to_wide(path).unwrap();
        let mut dacl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        // SAFETY: all output pointers refer to writable local storage and the
        // path buffer is NUL-terminated.
        let status = unsafe {
            GetNamedSecurityInfoW(
                path_w.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0, "GetNamedSecurityInfoW failed: {status}");
        assert!(!descriptor.is_null());

        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: descriptor was allocated and initialized by
        // GetNamedSecurityInfoW above.
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        assert_eq!(control & SE_DACL_PROTECTED != 0, protected);

        let mut dacl_present = 0_i32;
        let mut dacl_defaulted = 0_i32;
        // SAFETY: descriptor and dacl point to storage returned by the
        // successful GetNamedSecurityInfoW call.
        assert_ne!(
            unsafe {
                GetSecurityDescriptorDacl(
                    descriptor,
                    &mut dacl_present,
                    &mut dacl,
                    &mut dacl_defaulted,
                )
            },
            0
        );
        assert_ne!(dacl_present, 0);
        assert!(!dacl.is_null());

        let mut size = ACL_SIZE_INFORMATION::default();
        // SAFETY: dacl points to the descriptor's valid DACL and size is a
        // writable output buffer of the documented type.
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    &mut size as *mut ACL_SIZE_INFORMATION as *mut c_void,
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        assert_eq!(size.AceCount, 1);

        let mut ace = ptr::null_mut();
        // SAFETY: dacl is valid and ace is a writable output pointer.
        assert_ne!(unsafe { GetAce(dacl, 0, &mut ace) }, 0);
        // SAFETY: GetAce returned a valid ACCESS_ALLOWED_ACE pointer for ACE 0.
        let ace = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(ace.Header.AceFlags as u32, expected_flags);
        if inherited {
            assert_ne!(ace.Header.AceFlags as u32 & INHERITED_ACE, 0);
        } else {
            assert_eq!(ace.Header.AceFlags as u32 & INHERITED_ACE, 0);
        }
        assert_eq!(ace.Mask, FILE_ALL_ACCESS);
        let sid = current_user_sid().unwrap();
        let ace_sid = &ace.SidStart as *const u32 as *mut c_void;
        // SAFETY: ace_sid points into the validated ACE and sid points to an
        // owned current-user SID buffer.
        assert_ne!(unsafe { EqualSid(ace_sid, sid.as_ptr() as *mut c_void) }, 0);

        // SAFETY: descriptor was allocated by GetNamedSecurityInfoW and must
        // be released with LocalFree.
        unsafe {
            LocalFree(descriptor as HLOCAL);
        }
    }
}
