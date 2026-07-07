pub fn shell_escape(s: &str) -> String {
    if s.contains('\0') {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_')
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Expand a leading `~/` to the user's home directory.
pub fn shellexpand(path: &str) -> String {
    if path.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        path.replacen("~", &home, 1)
    } else {
        path.to_string()
    }
}

pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path cannot be empty".to_string());
    }
    if path.contains('\0') {
        return Err("path contains null byte".to_string());
    }
    if path.contains("..") {
        return Err("path traversal not allowed".to_string());
    }
    if path.bytes().any(|b| b < 0x20 && b != b'\t') {
        return Err("path contains control characters".to_string());
    }
    Ok(())
}
