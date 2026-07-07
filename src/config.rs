pub struct AppConfig {
    pub default_cols: u16,
    pub default_rows: u16,
    pub data_dir: String,
    pub pepper: String,
    pub jwt_secret: Vec<u8>,
}
