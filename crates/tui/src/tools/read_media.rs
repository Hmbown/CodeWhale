//! `read_media` tool — safe, first-class image reading and preprocessing.
//!
//! Provides bounded decoding, decompression-bomb protection, active route
//! vision-capability checks, crop region extraction, resolution detail modes,
//! and typed receipt metadata without leaking credentials.

use std::io::Cursor;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use codewhale_config::route::CapabilityState;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, RichToolResult, ToolCapability, ToolContext, ToolError, ToolResult,
    ToolResultContentBlock, ToolSpec, optional_str, required_str, type_mismatch,
};

/// Maximum source image size before decoding (20 MiB).
pub const MAX_SOURCE_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Maximum width or height admitted for an input image (8192 px).
pub const MAX_IMAGE_DIMENSION: u32 = 8192;

/// Maximum total pixels admitted before decoding is aborted (~33.5 megapixels).
pub const MAX_IMAGE_PIXELS: u64 = 33_554_432;

/// Memory allocation limit for image decoding (64 MiB).
pub const MAX_DECODE_ALLOC_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum inline image payload admitted on the wire (5 MiB).
pub const MAX_WIRE_IMAGE_BYTES: usize = crate::image_attach::MAX_IMAGE_BYTES;

/// Resolution/detail preference for image processing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailMode {
    #[default]
    Auto,
    Low,
    High,
    Original,
}

impl DetailMode {
    fn from_str_opt(s: Option<&str>) -> Result<Self, ToolError> {
        match s {
            None | Some("auto") => Ok(Self::Auto),
            Some("low") => Ok(Self::Low),
            Some("high") => Ok(Self::High),
            Some("original") | Some("full") => Ok(Self::Original),
            Some(other) => Err(ToolError::invalid_input(format!(
                "invalid detail mode '{other}'; expected 'auto', 'low', 'high', or 'original'"
            ))),
        }
    }

    fn max_dimension(self) -> u32 {
        match self {
            Self::Low => 1024,
            Self::Auto => 2048,
            Self::High | Self::Original => 4096,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::High => "high",
            Self::Original => "original",
        }
    }
}

/// Optional bounding box for cropping an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CropRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl CropRegion {
    fn parse_from_value(value: Option<&Value>) -> Result<Option<Self>, ToolError> {
        let Some(val) = value else {
            return Ok(None);
        };
        if val.is_null() {
            return Ok(None);
        }
        let obj = val
            .as_object()
            .ok_or_else(|| type_mismatch("crop", val, "an object with x, y, width, and height"))?;

        let extract_u32 = |field: &str| -> Result<u32, ToolError> {
            let num_val = obj.get(field).ok_or_else(|| {
                ToolError::invalid_input(format!("crop missing required field '{field}'"))
            })?;
            if let Some(n) = num_val.as_u64() {
                u32::try_from(n).map_err(|_| {
                    ToolError::invalid_input(format!(
                        "crop field '{field}' is out of range for u32"
                    ))
                })
            } else if let Some(n) = num_val.as_i64() {
                if n < 0 {
                    return Err(ToolError::invalid_input(format!(
                        "crop field '{field}' must be non-negative; got {n}"
                    )));
                }
                u32::try_from(n).map_err(|_| {
                    ToolError::invalid_input(format!(
                        "crop field '{field}' is out of range for u32"
                    ))
                })
            } else {
                Err(type_mismatch(
                    &format!("crop.{field}"),
                    num_val,
                    "an integer",
                ))
            }
        };

        let x = extract_u32("x")?;
        let y = extract_u32("y")?;
        let width = extract_u32("width")?;
        let height = extract_u32("height")?;

        if width == 0 || height == 0 {
            return Err(ToolError::invalid_input(
                "crop width and height must be greater than 0",
            ));
        }

        Ok(Some(Self {
            x,
            y,
            width,
            height,
        }))
    }
}

/// The first-class `read_media` tool.
pub struct ReadMediaTool;

impl Default for ReadMediaTool {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ToolSpec for ReadMediaTool {
    fn name(&self) -> &'static str {
        "read_media"
    }

    fn description(&self) -> &'static str {
        "Read an image file (PNG, JPEG, GIF, WebP) into context for multimodal/vision inspection, with optional crop region and detail level. Safe, bounded decode with pixel and byte guards."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image file (relative to workspace or absolute). PNG, JPEG, GIF, and WebP are supported."
                },
                "crop": {
                    "type": "object",
                    "description": "Optional bounding box to crop [x, y, width, height] in pixel coordinates.",
                    "properties": {
                        "x": {
                            "type": "integer",
                            "description": "Left coordinate (X) of the crop region in pixels (0-based)."
                        },
                        "y": {
                            "type": "integer",
                            "description": "Top coordinate (Y) of the crop region in pixels (0-based)."
                        },
                        "width": {
                            "type": "integer",
                            "description": "Width of the crop region in pixels (must be > 0)."
                        },
                        "height": {
                            "type": "integer",
                            "description": "Height of the crop region in pixels (must be > 0)."
                        }
                    },
                    "required": ["x", "y", "width", "height"]
                },
                "detail": {
                    "type": "string",
                    "enum": ["auto", "low", "high", "original"],
                    "description": "Resolution/detail preference. 'auto' (default) downscales large images to max 2048px; 'low' to max 1024px; 'high' / 'original' preserves resolution up to max 4096px within the 5 MiB payload limit."
                }
            },
            "required": ["path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    fn defer_loading(&self) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        self.execute_rich(input, context)
            .await
            .map(RichToolResult::into_result)
    }

    async fn execute_rich(
        &self,
        input: Value,
        context: &ToolContext,
    ) -> Result<RichToolResult, ToolError> {
        execute_read_media(input, context).await
    }
}

/// Execute the `read_media` tool logic.
pub(crate) async fn execute_read_media(
    input: Value,
    context: &ToolContext,
) -> Result<RichToolResult, ToolError> {
    // 0. Check cancellation early (before any path resolution, capability checks, or I/O)
    if context
        .cancel_token
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Err(ToolError::cancelled("Operation aborted"));
    }

    let path_str = required_str(&input, "path")?;
    let detail_str = optional_str(&input, "detail")?;
    let detail_mode = DetailMode::from_str_opt(detail_str)?;
    let crop_region = CropRegion::parse_from_value(input.get("crop"))?;

    // 1. Check active route vision capability
    if context.route_capabilities.image_input == CapabilityState::Unsupported {
        return Err(ToolError::execution_failed(
            "read_media: the active model route does not support image input. Switch to a route marked vision-capable with /model, or configure the route's image_input capability, then try again.",
        ));
    }

    // 2. Resolve path and protect credentials (including symlink/canonicalization escapes)
    // S1/F2: the deny-list check on the caller's raw spelling runs BEFORE
    // `resolve_path` — resolution canonicalizes a workspace symlink to the
    // secret's real location, and an error raised only afterwards would name
    // that location. `read_guard::check` canonicalizes internally, so the raw
    // spelling still matches by target; the resolved check below remains as
    // defense in depth.
    crate::tools::file::enforce_read_denylist(std::path::Path::new(path_str), "read_media")?;
    let file_path = context.resolve_path(path_str)?;
    if crate::tools::file::is_codewhale_credential_path(&file_path) {
        return Err(ToolError::permission_denied(
            "read_media cannot read Codewhale configuration or credential-store files; use `codewhale config list` or `codewhale auth status` for safe inspection",
        ));
    }
    crate::tools::file::enforce_read_denylist(&file_path, "read_media")?;

    // Check cancellation immediately before dispatching blocking I/O and decode work
    if context
        .cancel_token
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Err(ToolError::cancelled("Operation aborted"));
    }

    let file_path_clone = file_path.clone();
    let processed = tokio::task::spawn_blocking(move || {
        process_media_file(&file_path_clone, crop_region, detail_mode)
    })
    .await
    .map_err(|join_err| {
        ToolError::execution_failed(format!("read_media task failed: {join_err}"))
    })??;

    // Check cancellation after await
    if context
        .cancel_token
        .as_ref()
        .is_some_and(tokio_util::sync::CancellationToken::is_cancelled)
    {
        return Err(ToolError::cancelled("Operation aborted"));
    }

    // 10. Construct receipt and typed metadata without credentials
    context.note_file_read(&file_path);

    let crop_summary = if let Some(crop) = crop_region {
        format!(
            "region [x: {}, y: {}, w: {}, h: {}]",
            crop.x, crop.y, crop.width, crop.height
        )
    } else {
        "none".to_string()
    };

    let content_text = format!(
        "Read media file: {path_str} [image/png]\n\
         Original dimensions: {orig_width}x{orig_height} ({mime_type})\n\
         Processed dimensions: {final_width}x{final_height}\n\
         Crop: {crop_summary}\n\
         Detail: {}\n\
         Size: {} (source) -> {} (wire payload)",
        detail_mode.as_str(),
        human_bytes(processed.source_bytes),
        human_bytes(processed.encoded_bytes),
        orig_width = processed.orig_width,
        orig_height = processed.orig_height,
        mime_type = processed.mime_type,
        final_width = processed.final_width,
        final_height = processed.final_height,
    );

    let metadata_json = json!({
        "path": path_str,
        "mime_type": "image/png",
        "original_width": processed.orig_width,
        "original_height": processed.orig_height,
        "original_mime_type": processed.mime_type,
        "width": processed.final_width,
        "height": processed.final_height,
        "cropped": processed.crop_applied,
        "crop": crop_region.map(|c| json!({
            "x": c.x,
            "y": c.y,
            "width": c.width,
            "height": c.height
        })),
        "detail": detail_mode.as_str(),
        "source_bytes": processed.source_bytes,
        "encoded_bytes": processed.encoded_bytes
    });

    Ok(RichToolResult::with_content_blocks(
        ToolResult::success(content_text).with_metadata(metadata_json),
        vec![ToolResultContentBlock::Image {
            mime_type: "image/png".to_string(),
            data: processed.base64_payload,
        }],
    ))
}

struct ProcessedMedia {
    base64_payload: String,
    orig_width: u32,
    orig_height: u32,
    mime_type: &'static str,
    final_width: u32,
    final_height: u32,
    crop_applied: bool,
    source_bytes: usize,
    encoded_bytes: usize,
}

fn process_media_file(
    file_path: &std::path::Path,
    crop_region: Option<CropRegion>,
    detail_mode: DetailMode,
) -> Result<ProcessedMedia, ToolError> {
    // 3. Inspect metadata and check file bounds
    if !file_path.exists() {
        return Err(ToolError::execution_failed(format!(
            "read_media: image file does not exist: {}",
            file_path.display()
        )));
    }

    let meta = std::fs::metadata(file_path).map_err(|e| {
        ToolError::execution_failed(format!(
            "read_media: failed to inspect {}: {e}",
            file_path.display()
        ))
    })?;

    if meta.is_dir() {
        return Err(ToolError::execution_failed(format!(
            "read_media: path is a directory, not an image file: {}",
            file_path.display()
        )));
    }

    let file_len = meta.len();
    if file_len == 0 {
        return Err(ToolError::execution_failed(format!(
            "read_media: image file is empty (0 bytes): {}",
            file_path.display()
        )));
    }

    if file_len > MAX_SOURCE_IMAGE_BYTES as u64 {
        return Err(ToolError::execution_failed(format!(
            "read_media: image file size ({}) exceeds the maximum source limit of {}. Downscale or crop the file first.",
            human_bytes(file_len as usize),
            human_bytes(MAX_SOURCE_IMAGE_BYTES)
        )));
    }

    // 4. Read source bytes
    let raw_bytes = std::fs::read(file_path).map_err(|e| {
        ToolError::execution_failed(format!(
            "read_media: failed to read {}: {e}",
            file_path.display()
        ))
    })?;

    if raw_bytes.is_empty() {
        return Err(ToolError::execution_failed(format!(
            "read_media: image file is empty (0 bytes): {}",
            file_path.display()
        )));
    }

    if raw_bytes.len() > MAX_SOURCE_IMAGE_BYTES {
        return Err(ToolError::execution_failed(format!(
            "read_media: image file size ({}) exceeds the maximum source limit of {}. Downscale or crop the file first.",
            human_bytes(raw_bytes.len()),
            human_bytes(MAX_SOURCE_IMAGE_BYTES)
        )));
    }

    // 5. Sniff format and guard against non-images or rejected formats
    let sniffed_mime = crate::image_attach::sniff_media_type(&raw_bytes);
    let mime_type = match sniffed_mime {
        Some(m) => m,
        None => {
            if let Some(rejected) = crate::image_attach::detect_rejected_format(&raw_bytes) {
                return Err(ToolError::execution_failed(format!(
                    "read_media: {rejected} format is not directly supported by vision models. Convert it to PNG, JPEG, GIF, or WebP first."
                )));
            }
            return Err(ToolError::execution_failed(format!(
                "read_media: file is not a recognized or supported image format (expected PNG, JPEG, GIF, or WebP): {}",
                file_path.display()
            )));
        }
    };

    // 6. Bounded decoding with decompression-bomb guards
    let (processed_image, orig_width, orig_height) = decode_and_guard_image(&raw_bytes)?;

    // 7. Apply crop if requested
    let (cropped_image, crop_applied) = if let Some(crop) = crop_region {
        let crop_right = crop.x.checked_add(crop.width);
        let crop_bottom = crop.y.checked_add(crop.height);
        if crop_right.is_none_or(|right| right > orig_width)
            || crop_bottom.is_none_or(|bottom| bottom > orig_height)
        {
            return Err(ToolError::invalid_input(format!(
                "read_media: crop region [x: {}, y: {}, width: {}, height: {}] is out of bounds for image dimensions {}x{}",
                crop.x, crop.y, crop.width, crop.height, orig_width, orig_height
            )));
        }
        let cropped =
            image::imageops::crop_imm(&processed_image, crop.x, crop.y, crop.width, crop.height)
                .to_image();
        (DynamicImage::ImageRgba8(cropped), true)
    } else {
        (processed_image, false)
    };

    // 8. Apply detail resolution resizing
    let (current_w, current_h) = cropped_image.dimensions();
    let max_target = detail_mode.max_dimension();
    let final_image = if current_w > max_target || current_h > max_target {
        cropped_image.resize(max_target, max_target, FilterType::Lanczos3)
    } else {
        cropped_image
    };

    // 9. Re-encode to standard PNG within the wire budget
    let (final_bytes, final_width, final_height) =
        encode_to_bounded_png(&final_image, MAX_WIRE_IMAGE_BYTES)?;

    let source_bytes = raw_bytes.len();
    let encoded_bytes = final_bytes.len();
    let base64_payload = BASE64.encode(&final_bytes);

    Ok(ProcessedMedia {
        base64_payload,
        orig_width,
        orig_height,
        mime_type,
        final_width,
        final_height,
        crop_applied,
        source_bytes,
        encoded_bytes,
    })
}

fn decode_and_guard_image(bytes: &[u8]) -> Result<(DynamicImage, u32, u32), ToolError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| {
            ToolError::execution_failed(format!("read_media: failed to detect format: {e}"))
        })?;

    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    reader.limits(limits);

    // Read header/dimensions first
    let (width, height) = reader.into_dimensions().map_err(|e| {
        ToolError::execution_failed(format!(
            "read_media: decompression bomb guard or invalid header detected: {e}"
        ))
    })?;

    let total_pixels = (width as u64) * (height as u64);
    if total_pixels > MAX_IMAGE_PIXELS
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
    {
        return Err(ToolError::execution_failed(format!(
            "read_media: decompression bomb guard triggered: image dimensions ({width}x{height}, {total_pixels} pixels) exceed safe limits (max {MAX_IMAGE_DIMENSION}x{MAX_IMAGE_DIMENSION} / {MAX_IMAGE_PIXELS} pixels). Please downscale or crop the image first."
        )));
    }

    // Decode full dynamic image with limits
    let mut decode_reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| {
            ToolError::execution_failed(format!("read_media: failed to read image: {e}"))
        })?;
    let mut decode_limits = Limits::default();
    decode_limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    decode_limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    decode_limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    decode_reader.limits(decode_limits);

    let dynamic_img = decode_reader.decode().map_err(|e| {
        ToolError::execution_failed(format!("read_media: failed to decode image: {e}"))
    })?;

    Ok((dynamic_img, width, height))
}

fn encode_to_bounded_png(
    image: &DynamicImage,
    max_bytes: usize,
) -> Result<(Vec<u8>, u32, u32), ToolError> {
    let mut current = image.clone();
    let mut attempts = 0;

    loop {
        let (w, h) = current.dimensions();
        let mut buffer = Cursor::new(Vec::new());
        current
            .write_to(&mut buffer, ImageFormat::Png)
            .map_err(|e| {
                ToolError::execution_failed(format!("read_media: failed to encode PNG: {e}"))
            })?;

        let encoded = buffer.into_inner();
        if encoded.len() <= max_bytes || attempts >= 4 {
            if encoded.len() > max_bytes {
                return Err(ToolError::execution_failed(format!(
                    "read_media: encoded image size ({}) exceeds the {} wire limit even after compression",
                    human_bytes(encoded.len()),
                    human_bytes(max_bytes)
                )));
            }
            return Ok((encoded, w, h));
        }

        // Downscale progressively to fit within the 5 MiB ceiling
        attempts += 1;
        let next_w = ((w as f64) * 0.75).max(32.0) as u32;
        let next_h = ((h as f64) * 0.75).max(32.0) as u32;
        current = current.resize(next_w, next_h, FilterType::Lanczos3);
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;
    use tempfile::tempdir;

    fn create_test_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        cursor.into_inner()
    }

    fn create_test_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(width, height, image::Rgb([120, 200, 50]));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Jpeg).unwrap();
        cursor.into_inner()
    }

    #[tokio::test]
    async fn read_media_spec_metadata_and_capabilities() {
        let tool = ReadMediaTool;
        assert_eq!(tool.name(), "read_media");
        assert!(tool.capabilities().contains(&ToolCapability::ReadOnly));
        assert!(tool.capabilities().contains(&ToolCapability::Sandboxable));
        assert!(tool.supports_parallel());
        assert!(tool.defer_loading());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
    }

    #[tokio::test]
    async fn read_media_success_png_roundtrip() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("test.png");
        let png_data = create_test_png(100, 50, [255, 0, 0, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let mut ctx = ToolContext::new(dir.path());
        ctx.route_capabilities.image_input = CapabilityState::Supported;

        let tool = ReadMediaTool;
        let input = json!({
            "path": "test.png",
            "detail": "auto"
        });

        let rich = tool.execute_rich(input, &ctx).await.unwrap();
        assert!(rich.success);
        assert_eq!(rich.content_blocks.len(), 1);
        let ToolResultContentBlock::Image { mime_type, data } = &rich.content_blocks[0];
        assert_eq!(mime_type, "image/png");
        assert!(!data.is_empty());

        let meta = rich.metadata.as_ref().unwrap();
        assert_eq!(meta["path"], "test.png");
        assert_eq!(meta["original_width"], 100);
        assert_eq!(meta["original_height"], 50);
        assert_eq!(meta["width"], 100);
        assert_eq!(meta["height"], 50);
        assert_eq!(meta["cropped"], false);
        assert_eq!(meta["detail"], "auto");
    }

    #[tokio::test]
    async fn read_media_success_jpeg_decoded_and_reencoded_to_png() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("photo.jpg");
        let jpeg_data = create_test_jpeg(80, 60);
        std::fs::write(&img_path, &jpeg_data).unwrap();

        let mut ctx = ToolContext::new(dir.path());
        ctx.route_capabilities.image_input = CapabilityState::Supported;

        let tool = ReadMediaTool;
        let input = json!({
            "path": "photo.jpg",
            "detail": "low"
        });

        let rich = tool.execute_rich(input, &ctx).await.unwrap();
        assert!(rich.success);
        let ToolResultContentBlock::Image { mime_type, data } = &rich.content_blocks[0];
        assert_eq!(mime_type, "image/png");
        assert!(!data.is_empty());

        let meta = rich.metadata.as_ref().unwrap();
        assert_eq!(meta["original_width"], 80);
        assert_eq!(meta["original_height"], 60);
        assert_eq!(meta["original_mime_type"], "image/jpeg");
    }

    #[tokio::test]
    async fn read_media_crop_region_bounds_and_execution() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("grid.png");
        let png_data = create_test_png(200, 100, [0, 255, 0, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let mut ctx = ToolContext::new(dir.path());
        ctx.route_capabilities.image_input = CapabilityState::Supported;

        let tool = ReadMediaTool;

        // 1. Valid crop
        let valid_input = json!({
            "path": "grid.png",
            "crop": {
                "x": 10,
                "y": 20,
                "width": 50,
                "height": 40
            }
        });
        let rich = tool.execute_rich(valid_input, &ctx).await.unwrap();
        assert!(rich.success);
        let meta = rich.metadata.as_ref().unwrap();
        assert_eq!(meta["cropped"], true);
        assert_eq!(meta["width"], 50);
        assert_eq!(meta["height"], 40);

        // 2. Out-of-bounds crop
        let oob_input = json!({
            "path": "grid.png",
            "crop": {
                "x": 180,
                "y": 20,
                "width": 50,
                "height": 40
            }
        });
        let err = tool.execute_rich(oob_input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("out of bounds"));

        // 3. Zero-dimension crop
        let zero_input = json!({
            "path": "grid.png",
            "crop": {
                "x": 10,
                "y": 10,
                "width": 0,
                "height": 10
            }
        });
        let err = tool.execute_rich(zero_input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("greater than 0"));

        // Adversarial coordinates must be rejected rather than wrapping in
        // release builds or panicking in debug builds.
        for overflow_input in [
            json!({
                "path": "grid.png",
                "crop": {
                    "x": u32::MAX,
                    "y": 0,
                    "width": 2,
                    "height": 1
                }
            }),
            json!({
                "path": "grid.png",
                "crop": {
                    "x": 0,
                    "y": u32::MAX,
                    "width": 1,
                    "height": 2
                }
            }),
        ] {
            let err = tool.execute_rich(overflow_input, &ctx).await.unwrap_err();
            assert!(err.to_string().contains("out of bounds"), "{err}");
        }
    }

    #[tokio::test]
    async fn read_media_detail_modes_and_resizing() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("large.png");
        let png_data = create_test_png(3000, 1500, [0, 0, 255, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let mut ctx = ToolContext::new(dir.path());
        ctx.route_capabilities.image_input = CapabilityState::Supported;
        let tool = ReadMediaTool;

        // Low detail -> max 1024
        let low_input = json!({ "path": "large.png", "detail": "low" });
        let rich_low = tool.execute_rich(low_input, &ctx).await.unwrap();
        let meta_low = rich_low.metadata.as_ref().unwrap();
        assert_eq!(meta_low["width"], 1024);
        assert_eq!(meta_low["height"], 512);

        // Auto detail -> max 2048
        let auto_input = json!({ "path": "large.png", "detail": "auto" });
        let rich_auto = tool.execute_rich(auto_input, &ctx).await.unwrap();
        let meta_auto = rich_auto.metadata.as_ref().unwrap();
        assert_eq!(meta_auto["width"], 2048);
        assert_eq!(meta_auto["height"], 1024);

        // Original detail -> within 4096, keeps 3000x1500
        let orig_input = json!({ "path": "large.png", "detail": "original" });
        let rich_orig = tool.execute_rich(orig_input, &ctx).await.unwrap();
        let meta_orig = rich_orig.metadata.as_ref().unwrap();
        assert_eq!(meta_orig["width"], 3000);
        assert_eq!(meta_orig["height"], 1500);
    }

    #[tokio::test]
    async fn read_media_route_capability_unsupported_fails_actionable() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("img.png");
        let png_data = create_test_png(10, 10, [1, 2, 3, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let mut ctx = ToolContext::new(dir.path());
        ctx.route_capabilities.image_input = CapabilityState::Unsupported;

        let tool = ReadMediaTool;
        let input = json!({ "path": "img.png" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("active model route does not support image input"));
        assert!(err_msg.contains("/model"));
        assert!(err_msg.contains("image_input"));
        assert!(!err_msg.contains("deepseek-v4-pro"));
    }

    #[tokio::test]
    async fn read_media_missing_file_fails_actionable() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let input = json!({ "path": "nonexistent.png" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("image file does not exist"));
    }

    #[tokio::test]
    async fn read_media_empty_file_fails_actionable() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("empty.png");
        std::fs::write(&img_path, b"").unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let input = json!({ "path": "empty.png" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("image file is empty"));
    }

    #[tokio::test]
    async fn read_media_corrupted_or_non_image_fails_actionable() {
        let dir = tempdir().unwrap();
        let bad_path = dir.path().join("corrupted.png");
        std::fs::write(&bad_path, b"not a real image payload at all").unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let input = json!({ "path": "corrupted.png" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("not a recognized or supported image format")
        );
    }

    #[tokio::test]
    async fn read_media_rejected_format_svg_fails_with_conversion_hint() {
        let dir = tempdir().unwrap();
        let svg_path = dir.path().join("vector.svg");
        std::fs::write(&svg_path, b"<svg><circle r='10'/></svg>").unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let input = json!({ "path": "vector.svg" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("SVG format is not directly supported by vision models"));
        assert!(msg.contains("Convert it to PNG, JPEG, GIF, or WebP"));
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn read_media_credential_path_is_denied() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
        let _config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _legacy_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        std::fs::write(tmp.path().join("config.toml"), "api_key = \"secret\"\n")
            .expect("write config");

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let tool = ReadMediaTool;
        let input = json!({ "path": "config.toml" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot read Codewhale configuration or credential-store"),
            "{}",
            err
        );
    }

    #[allow(clippy::await_holding_lock)]
    #[cfg(unix)]
    #[tokio::test]
    async fn read_media_credential_symlink_escape_is_denied() {
        let _env_lock = crate::test_support::lock_test_env();
        let home_tmp = tempdir().expect("home tempdir");
        let ws_tmp = tempdir().expect("workspace tempdir");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home_tmp.path());
        let _config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _legacy_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        let real_config = home_tmp.path().join("config.toml");
        std::fs::write(&real_config, "api_key = \"secret_in_home\"\n").expect("write config");

        // Create a symlink in the workspace pointing to the credentials
        let symlink_path = ws_tmp.path().join("fake_image.png");
        std::os::unix::fs::symlink(&real_config, &symlink_path).expect("create symlink");

        {
            // Case 1: Default context (no symlink follow outside workspace) -> path escape error
            let ctx_default = ToolContext::new(ws_tmp.path().to_path_buf());
            let tool = ReadMediaTool;
            let input = json!({ "path": "fake_image.png" });
            let err_default = tool
                .execute_rich(input.clone(), &ctx_default)
                .await
                .unwrap_err();
            let msg_default = err_default.to_string();
            assert!(
                msg_default.contains("escapes workspace") || msg_default.contains("credential"),
                "default policy must reject symlink outside workspace: {msg_default}"
            );

            // Case 2: follow_symlinks enabled -> credential guard must still deny read
            let ctx_follow =
                ToolContext::new(ws_tmp.path().to_path_buf()).with_follow_symlinks(true);
            let err_follow = tool.execute_rich(input, &ctx_follow).await.unwrap_err();
            assert!(
                err_follow
                    .to_string()
                    .contains("cannot read Codewhale configuration or credential-store"),
                "follow_symlinks policy must catch canonical credential path: {}",
                err_follow
            );
        }
    }

    #[tokio::test]
    async fn read_media_path_escape_is_rejected() {
        let dir = tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let input = json!({ "path": "../../etc/shadow" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        assert!(
            err.to_string().contains("escapes workspace") || err.to_string().contains("permission"),
            "{}",
            err
        );
    }

    #[tokio::test]
    async fn read_media_respects_cancel_token() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("cancel.png");
        let png_data = create_test_png(10, 10, [1, 2, 3, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();
        let mut ctx = ToolContext::new(dir.path()).with_cancel_token(cancel_token);
        ctx.route_capabilities.image_input = CapabilityState::Supported;

        let tool = ReadMediaTool;
        let err = tool
            .execute_rich(json!({ "path": "cancel.png" }), &ctx)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("aborted") || msg.contains("cancel"), "{msg}");
    }

    #[tokio::test]
    async fn read_media_cancellation_before_dispatch_fails_without_read() {
        let dir = tempdir().unwrap();
        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();
        let mut ctx = ToolContext::new(dir.path()).with_cancel_token(cancel_token);
        ctx.route_capabilities.image_input = CapabilityState::Supported;

        let tool = ReadMediaTool;
        // A non-existent file path would normally fail with "image file does not exist",
        // but when cancelled before dispatch, it must abort with Cancelled without touching disk.
        let input = json!({ "path": "nonexistent_before_dispatch.png" });
        let err = tool.execute_rich(input, &ctx).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("aborted") || msg.contains("cancel"),
            "expected cancellation error before dispatch, got: {msg}"
        );
        assert!(
            !msg.contains("does not exist"),
            "should not reach filesystem checks when cancelled before dispatch"
        );
    }

    #[tokio::test]
    async fn read_media_provider_request_wiring_integration() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("wire.png");
        let png_data = create_test_png(40, 40, [10, 20, 30, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let rich = tool
            .execute_rich(json!({ "path": "wire.png" }), &ctx)
            .await
            .unwrap();

        let ToolResultContentBlock::Image { mime_type, data } = &rich.content_blocks[0];

        // 1. Check Anthropic tool result wiring
        let anthropic_content = crate::client::anthropic_tool_result_content_for_test(
            &rich.content,
            Some(&[json!({
                "type": "image",
                "mime_type": mime_type,
                "data": data
            })]),
        );
        let blocks = anthropic_content
            .as_array()
            .expect("anthropic content array");
        assert!(
            blocks
                .iter()
                .any(|b| b["type"] == "image" && b["source"]["media_type"] == "image/png")
        );

        // 2. Check OpenAI Responses tool output wiring
        let responses_content = crate::client::responses_tool_output_for_test(
            &rich.content,
            Some(&[json!({
                "type": "image",
                "mime_type": mime_type,
                "data": data
            })]),
        );
        let resp_blocks = responses_content
            .as_array()
            .expect("responses content array");
        assert!(resp_blocks.iter().any(|b| b["type"] == "input_image"));

        // 3. Check Chat Completions provider request body wiring
        let messages = vec![
            crate::models::Message {
                role: Role::Assistant,
                content: vec![crate::models::ContentBlock::ToolUse {
                    id: "call_read_media".to_string(),
                    name: "read_media".to_string(),
                    input: json!({ "path": "wire.png" }),
                    caller: None,
                    thought_signature: None,
                }],
            },
            crate::models::Message {
                role: Role::User,
                content: vec![crate::models::ContentBlock::ToolResult {
                    tool_use_id: "call_read_media".to_string(),
                    content: rich.content.clone(),
                    is_error: None,
                    content_blocks: Some(vec![json!({
                        "type": "image",
                        "mime_type": mime_type,
                        "data": data
                    })]),
                }],
            },
        ];
        let chat_msgs = crate::client::chat_messages_for_test(&messages);
        let tool_msg = chat_msgs
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == "call_read_media")
            .expect("tool response message");
        assert!(
            tool_msg["content"]
                .as_str()
                .unwrap()
                .contains("Read media file")
        );
        let follow_up_user = chat_msgs
            .iter()
            .find(|m| m["role"] == "user" && m["content"].is_array())
            .expect("follow-up user message carrying tool image");
        let parts = follow_up_user["content"].as_array().unwrap();
        assert!(
            parts.iter().any(|part| {
                part["type"] == "image_url"
                    && part["image_url"]["url"]
                        .as_str()
                        .is_some_and(|u| u.starts_with("data:image/png;base64,"))
            }),
            "expected image_url part in chat completions follow-up message: {parts:?}"
        );

        // 4. Check Chat Completions provider tool result refs helper
        let blocks = [json!({
            "type": "image",
            "mime_type": mime_type,
            "data": data
        })];
        let (image_ref, omitted) =
            crate::image_attach::provider_tool_result_image_refs(Some(&blocks));
        assert_eq!(omitted, 0);
        assert!(image_ref.is_some());
        let (sniffed_mime, payload) = image_ref.unwrap();
        assert_eq!(sniffed_mime, "image/png");
        assert_eq!(payload, data);
    }

    #[tokio::test]
    async fn read_media_decompression_bomb_rejected() {
        let dir = tempdir().unwrap();
        let bomb_path = dir.path().join("bomb.png");

        // Construct a synthetic PNG header with 10,000 x 10,000 dimensions (100 megapixels > 33.5 megapixel guard)
        // PNG signature + IHDR chunk (length 13, type IHDR, width, height, bit depth 8, color type 6, etc.)
        let mut fake_png = Vec::new();
        fake_png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        fake_png.extend_from_slice(&13_u32.to_be_bytes()); // IHDR length
        fake_png.extend_from_slice(b"IHDR");
        fake_png.extend_from_slice(&10_000_u32.to_be_bytes()); // width: 10,000
        fake_png.extend_from_slice(&10_000_u32.to_be_bytes()); // height: 10,000
        fake_png.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        fake_png.extend_from_slice(&[0, 0, 0, 0]); // CRC (dummy)
        std::fs::write(&bomb_path, &fake_png).unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let err = tool
            .execute_rich(json!({ "path": "bomb.png" }), &ctx)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("decompression bomb")
                || msg.contains("exceed safe limits")
                || msg.contains("guard triggered"),
            "{}",
            msg
        );
    }

    #[tokio::test]
    async fn read_media_gif_format_supported() {
        let dir = tempdir().unwrap();
        let gif_path = dir.path().join("anim.gif");
        let img = image::RgbaImage::from_pixel(30, 30, image::Rgba([100, 150, 200, 255]));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Gif).unwrap();
        std::fs::write(&gif_path, cursor.into_inner()).unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let rich = tool
            .execute_rich(json!({ "path": "anim.gif" }), &ctx)
            .await
            .unwrap();
        assert!(rich.success);
        let meta = rich.metadata.as_ref().unwrap();
        assert_eq!(meta["original_mime_type"], "image/gif");
        assert_eq!(meta["mime_type"], "image/png");
    }

    #[tokio::test]
    async fn read_media_webp_format_supported() {
        let dir = tempdir().unwrap();
        let webp_path = dir.path().join("image.webp");
        let webp_bytes = BASE64
            .decode("UklGRkoAAABXRUJQVlA4WAoAAAAQAAAAAAAAAAAAQUxQSAwAAAARBxAR/Q9ERP8DAABWUDggGAAAADABAJ0BKgEAAQABABwlpAADcAD+/gbQAA==")
            .expect("valid webp");
        std::fs::write(&webp_path, webp_bytes).unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let rich = tool
            .execute_rich(json!({ "path": "image.webp" }), &ctx)
            .await
            .unwrap();
        assert!(rich.success);
        let meta = rich.metadata.as_ref().unwrap();
        assert_eq!(meta["original_mime_type"], "image/webp");
        assert_eq!(meta["mime_type"], "image/png");
    }

    #[tokio::test]
    async fn read_media_supported_and_unknown_routes_admitted() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("check.png");
        let png_data = create_test_png(20, 20, [50, 50, 50, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let tool = ReadMediaTool;

        // 1. Supported route
        let mut ctx_sup = ToolContext::new(dir.path());
        ctx_sup.route_capabilities.image_input = CapabilityState::Supported;
        let res_sup = tool
            .execute_rich(json!({ "path": "check.png" }), &ctx_sup)
            .await;
        assert!(res_sup.is_ok());

        // Unknown deliberately matches the established attachment contract:
        // custom/self-hosted routes frequently lack modality metadata, so
        // only a known Unsupported verdict blocks this explicit user action
        // and the provider remains authoritative.
        let mut ctx_unk = ToolContext::new(dir.path());
        ctx_unk.route_capabilities.image_input = CapabilityState::Unknown;
        let res_unk = tool
            .execute_rich(json!({ "path": "check.png" }), &ctx_unk)
            .await;
        assert!(res_unk.is_ok());
    }

    #[tokio::test]
    async fn read_media_receipt_contains_no_credentials() {
        let dir = tempdir().unwrap();
        let img_path = dir.path().join("safe.png");
        let png_data = create_test_png(15, 15, [255, 255, 255, 255]);
        std::fs::write(&img_path, &png_data).unwrap();

        let ctx = ToolContext::new(dir.path());
        let tool = ReadMediaTool;
        let rich = tool
            .execute_rich(json!({ "path": "safe.png" }), &ctx)
            .await
            .unwrap();

        let meta = rich.metadata.as_ref().unwrap();
        let meta_str = meta.to_string();
        assert!(!meta_str.contains("key"));
        assert!(!meta_str.contains("secret"));
        assert!(!meta_str.contains("token"));
        assert!(!meta_str.contains("password"));
        assert!(!meta_str.contains("auth"));
    }
}
