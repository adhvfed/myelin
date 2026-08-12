pub fn assert_service_refuses_runtime_without_migration_credential(
    binary: &str,
    service: &str,
    downstream_markers: &[&str],
) {
    let output = std::process::Command::new(binary)
        .env("DATABASE_URL", "postgres://runtime.invalid/myelin")
        .env_remove("DATABASE_MIGRATION_URL")
        .env("S3_ENDPOINT", "http://storage.invalid")
        .env("S3_REGION", "fr-par")
        .env("S3_ACCESS_KEY", "test-access")
        .env("S3_SECRET_KEY", "test-secret")
        .env("S3_BUCKET", "test-bucket")
        .env("REDIS_URL", "redis://cache.invalid")
        .env("NATS_URL", "nats://bus.invalid")
        .env("MYELIN_REGION", "fr-par")
        .env_remove("MYELIN_OIDC_ISSUER")
        .env_remove("MYELIN_OIDC_AUDIENCE")
        .env_remove("MYELIN_OIDC_JWKS")
        .env_remove("MYELIN_OIDC_JWKS_FILE")
        .output()
        .unwrap_or_else(|error| panic!("{service} process must launch: {error}"));

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    for marker in downstream_markers {
        assert!(
            !stderr.contains(marker),
            "{service} reached downstream startup work `{marker}`: {stderr}"
        );
    }
}
