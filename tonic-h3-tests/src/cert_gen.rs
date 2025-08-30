pub fn make_test_cert(subject_alt_names: Vec<String>) -> (rcgen::Certificate, rcgen::KeyPair) {
    use rcgen::generate_simple_self_signed;
    let key_pair = generate_simple_self_signed(subject_alt_names).unwrap();
    (key_pair.cert, key_pair.signing_key)
}

/// Create cert files for test.
/// This may leave the certs behind after the test.
pub fn make_test_cert_files(
    test_name: &str,
    regen: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::io::Write;

    // Create a temporary directory
    let temp_dir = std::env::temp_dir()
        .join("tonic_h3_test_certs")
        .join(test_name);

    // remove and regenerate.
    if regen {
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp directory");
    }

    // Define file paths in temp directory
    let cert_path = temp_dir.join("cert.pem");
    let key_path = temp_dir.join("key.pem");
    if !key_path.exists() || !cert_path.exists() {
        let (cert, key) = make_test_cert(vec!["localhost".to_string(), "127.0.0.1".to_string()]);

        // Save certificate to file
        let mut cert_f = std::fs::File::create(&cert_path).expect("Failed to create cert file");
        cert_f
            .write_all(cert.pem().as_bytes())
            .expect("Failed to write cert");

        // Save private key to file
        let mut key_f = std::fs::File::create(&key_path).expect("Failed to create key file");
        key_f
            .write_all(key.serialize_pem().as_bytes())
            .expect("Failed to write key");
    }
    (cert_path, key_path)
}
