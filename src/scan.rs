//! Reading QR codes with the camera.

use istmo_barcode_scanner::{BarcodeScannerClient, ScanConfig};

/// Why the camera scanner did not return a code.
#[derive(Debug)]
pub enum ScanError {
    /// No camera or scanner on this device.
    Unavailable,
    /// The user refused camera access.
    Denied,
    Failed(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("no camera scanner"),
            Self::Denied => f.write_str("camera access denied"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ScanError {}

/// Scan one QR code with `prompt` over the camera. `Ok(None)` when the
/// user backs out.
///
/// # Errors
/// [`ScanError`] when there is no scanner, no permission, or it fails.
pub async fn scan_qr(prompt: &str) -> Result<Option<String>, ScanError> {
    let client = BarcodeScannerClient::acquire().map_err(|e| ScanError::Failed(e.to_string()))?;
    match client
        .scan(ScanConfig::qr(prompt))
        .await
        .map_err(istmo_barcode_scanner::ScanError::from)
    {
        Ok(code) => Ok(code.map(|c| c.text)),
        Err(istmo_barcode_scanner::ScanError::Unavailable) => Err(ScanError::Unavailable),
        Err(istmo_barcode_scanner::ScanError::PermissionDenied) => Err(ScanError::Denied),
        Err(istmo_barcode_scanner::ScanError::Backend(why)) => Err(ScanError::Failed(why)),
    }
}
