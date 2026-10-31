use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use minisign_verify::{PublicKey, Signature};

#[derive(Debug)]
enum VerifyFailure {
    InvalidSignature(String),
    Unverifiable(String),
}

fn verify_files(
    public_key_path: &Path,
    data_path: &Path,
    signature_path: &Path,
) -> Result<usize, VerifyFailure> {
    let public_key = PublicKey::from_file(public_key_path)
        .map_err(|error| VerifyFailure::Unverifiable(format!("public key: {error}")))?;
    let signature = Signature::from_file(signature_path)
        .map_err(|error| VerifyFailure::InvalidSignature(format!("signature: {error}")))?;
    let data = fs::read(data_path)
        .map_err(|error| VerifyFailure::Unverifiable(format!("data: {error}")))?;

    public_key
        .verify(&data, &signature, false)
        .map_err(|error| VerifyFailure::InvalidSignature(error.to_string()))?;

    Ok(data.len())
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        eprintln!("usage: verify-minisign <public-key-file> <data-file> <signature-file>");
        return ExitCode::from(2);
    }

    match verify_files(
        Path::new(&args[0]),
        Path::new(&args[1]),
        Path::new(&args[2]),
    ) {
        Ok(bytes) => {
            println!("PASS minisign-verify 0.2.5 allow_legacy=false algorithm=ED bytes={bytes}");
            ExitCode::SUCCESS
        }
        Err(VerifyFailure::InvalidSignature(detail)) => {
            eprintln!("INVALID_SIGNATURE: {detail}");
            ExitCode::from(1)
        }
        Err(VerifyFailure::Unverifiable(detail)) => {
            eprintln!("UNVERIFIABLE: {detail}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const PREHASHED_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
trusted comment: timestamp:1556193335\tfile:test\n\
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";
    const LEGACY_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\n\
RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\n\
trusted comment: timestamp:1555779966\tfile:test\n\
QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

    fn verify_bytes(data: &[u8], signature: &str) -> Result<(), VerifyFailure> {
        let public_key = PublicKey::from_base64(PUBLIC_KEY)
            .map_err(|error| VerifyFailure::Unverifiable(error.to_string()))?;
        let signature = Signature::decode(signature)
            .map_err(|error| VerifyFailure::InvalidSignature(error.to_string()))?;
        public_key
            .verify(data, &signature, false)
            .map_err(|error| VerifyFailure::InvalidSignature(error.to_string()))
    }

    fn corrupt_line(text: &str, line_number: usize) -> String {
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let line = &mut lines[line_number - 1];
        let index = line.len() / 2;
        let replacement = if line.as_bytes()[index] == b'A' {
            'B'
        } else {
            'A'
        };
        line.replace_range(index..index + 1, &replacement.to_string());
        lines.join("\n")
    }

    fn assert_invalid(result: Result<(), VerifyFailure>) {
        assert!(matches!(result, Err(VerifyFailure::InvalidSignature(_))));
    }

    #[test]
    fn accepts_the_fixed_prehashed_vector() {
        assert!(verify_bytes(b"test", PREHASHED_SIGNATURE).is_ok());
    }

    #[test]
    fn rejects_changed_file_bytes() {
        assert_invalid(verify_bytes(b"Test", PREHASHED_SIGNATURE));
    }

    #[test]
    fn rejects_a_legacy_algorithm() {
        assert_invalid(verify_bytes(b"test", LEGACY_SIGNATURE));
    }

    #[test]
    fn rejects_a_tampered_signature_payload() {
        assert_invalid(verify_bytes(b"test", &corrupt_line(PREHASHED_SIGNATURE, 2)));
    }

    #[test]
    fn rejects_a_tampered_trusted_comment() {
        let tampered = PREHASHED_SIGNATURE.replace("timestamp:1556193335", "timestamp:1556193336");
        assert_invalid(verify_bytes(b"test", &tampered));
    }

    #[test]
    fn rejects_a_tampered_global_signature() {
        assert_invalid(verify_bytes(b"test", &corrupt_line(PREHASHED_SIGNATURE, 4)));
    }

    #[test]
    fn rejects_malformed_signature_input() {
        assert_invalid(verify_bytes(b"test", "not a minisign signature"));
    }
}
