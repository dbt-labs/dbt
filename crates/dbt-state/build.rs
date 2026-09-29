use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from("proto");
    let proto_files = [
        proto_root.join("query_cache_protobuf/query_cache/shared.proto"),
        proto_root.join("query_cache_protobuf/query_cache/struct.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/clone_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/sql_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/execution_service.proto"),
        proto_root
            .join("query_cache_protobuf/query_cache/services/client_validation_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/client_telemetry_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/explain_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/health_service.proto"),
        proto_root.join("query_cache_protobuf/query_cache/services/selector_service.proto"),
    ];

    println!("cargo:rerun-if-changed=build.rs");
    for proto_file in &proto_files {
        println!("cargo:rerun-if-changed={}", proto_file.display());
    }

    tonic_prost_build::configure()
        .build_server(false)
        .boxed(".com.fivetran.query_cache.ExecutionRecord.input") // avoids a clippy warning about large enum variants
        .compile_protos(&proto_files, &[proto_root])?;

    Ok(())
}
