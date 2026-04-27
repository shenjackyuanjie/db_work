use super::{DISEASE_CLASSES, decode_image_data, preprocess_image_data, softmax, validate_climate};
use base64::Engine;
use image::{DynamicImage, ImageFormat, RgbImage};
use std::io::Cursor;

fn one_by_one_png_data_uri() -> String {
    let img = RgbImage::from_pixel(1, 1, image::Rgb([255, 255, 255]));
    let dyn_img = DynamicImage::ImageRgb8(img);
    let mut bytes = Vec::new();
    dyn_img
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode png should succeed");
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[test]
fn decode_data_uri_ok() {
    let data_uri = one_by_one_png_data_uri();
    let bytes = decode_image_data(&data_uri).expect("decode should succeed");
    assert!(!bytes.is_empty());
}

#[test]
fn decode_data_uri_with_spaces_and_uppercase_ok() {
    let data_uri = one_by_one_png_data_uri();
    let encoded = data_uri.split(',').nth(1).unwrap_or_default();
    let noisy = format!("  DATA:IMAGE/PNG;BASE64,{}  \n", encoded);
    let bytes = decode_image_data(&noisy).expect("decode with spaces should succeed");
    assert!(!bytes.is_empty());
}

#[test]
fn decode_octet_stream_data_uri_ok() {
    let data_uri = one_by_one_png_data_uri();
    let encoded = data_uri.split(',').nth(1).unwrap_or_default();
    let octet = format!("data:application/octet-stream;base64,{}", encoded);
    let bytes = decode_image_data(&octet).expect("decode octet-stream data uri should succeed");
    assert!(!bytes.is_empty());
}

#[test]
fn preprocess_to_nchw_224_ok() {
    let data_uri = one_by_one_png_data_uri();
    let tensor = preprocess_image_data(&data_uri).expect("preprocess should succeed");
    assert_eq!(tensor.shape(), &[1, 3, 224, 224]);
}

#[test]
fn softmax_sums_to_one() {
    let probs = softmax(&[1.0, 2.0, 3.0]);
    let sum: f32 = probs.iter().sum();
    assert!((sum - 1.0).abs() < 1e-5);
}

#[test]
fn climate_validation_skips_without_environment() {
    let result = validate_climate(
        &DISEASE_CLASSES,
        &[0.2, 0.6, 0.1, 0.1],
        "健康果树",
        60.0,
        None,
        None,
    );
    assert!(!result.used);
    assert_eq!(
        result.reason.as_deref(),
        Some("未提供温度或湿度，跳过温湿度校验。")
    );
}

#[test]
fn climate_validation_can_reweight_prediction() {
    let result = validate_climate(
        &DISEASE_CLASSES,
        &[0.31, 0.34, 0.30, 0.05],
        "健康果树",
        34.0,
        Some(28.0),
        Some(88.0),
    );
    assert!(result.used);
    assert_eq!(result.adjusted_predicted_class.as_deref(), Some("黄龙病"));
}