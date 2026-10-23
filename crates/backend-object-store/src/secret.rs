use std::path::Path;

/// Reads a credential from a file, e.g. a mounted Kubernetes Secret, ignoring
/// surrounding whitespace such as the trailing line break editors and `echo`
/// add.
pub fn read_secret(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path).map(|secret| secret.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_trimmed() {
        let dir = std::env::temp_dir().join(format!("yy-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("key");
        std::fs::write(&file, "c2VjcmV0\n").unwrap();

        assert_eq!(read_secret(&file).unwrap(), "c2VjcmV0");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(read_secret(Path::new("/nonexistent/secret")).is_err());
    }
}
