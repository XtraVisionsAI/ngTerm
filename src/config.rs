pub struct AppConfig {
    pub default_cols: u16,
    pub default_rows: u16,
    pub data_dir: String,
    pub pepper: String,
    pub jwt_secret: Vec<u8>,
    pub recording: crate::recording::RecordingConfig,
}

/// Hard resource limits. They bound what a single request or client can
/// make the server buffer or hold, independent of admin configuration.
pub mod limits {
    /// JSON API request bodies.
    pub const MAX_JSON_BODY_BYTES: usize = 1024 * 1024;
    /// `PUT /files/content` (editor save) bodies.
    pub const MAX_FILE_WRITE_BYTES: usize = 16 * 1024 * 1024;
    /// Multipart uploads; streamed to SFTP, so this is a sanity bound only.
    pub const MAX_UPLOAD_BYTES: usize = 4 * 1024 * 1024 * 1024;
    /// Largest file `GET /files/content` will load into memory for preview.
    pub const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
    /// Largest single WebSocket message accepted from a client.
    pub const MAX_WS_MESSAGE_BYTES: usize = 1024 * 1024;
    /// Live terminal sessions one user may hold at once.
    pub const MAX_SESSIONS_PER_USER: usize = 32;
}
