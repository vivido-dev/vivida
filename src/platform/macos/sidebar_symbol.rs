use std::sync::Arc;

use objc2_app_kit::{NSImage, NSImageSymbolConfiguration};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, ns_string};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use vivido::display::color::Rgb;

/// Rasterize the native SF Symbol once per display scale for the Vello chrome.
pub fn sidebar_symbol(pixel_size: u32, color: Rgb) -> Option<ImageData> {
    if !(1..=128).contains(&pixel_size) {
        return None;
    }
    let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        ns_string!("sidebar.left"),
        Some(ns_string!("Toggle sidebar")),
    )?;
    let configuration =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(f64::from(pixel_size), 0.0);
    let symbol = symbol.imageWithSymbolConfiguration(&configuration)?;
    let mut proposed = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(f64::from(pixel_size), f64::from(pixel_size)),
    );
    // SAFETY: the proposed rectangle is live and writable; no hints are supplied.
    let image = unsafe { symbol.CGImageForProposedRect_context_hints(&mut proposed, None, None) }?;
    let width = CGImage::width(Some(&image));
    let height = CGImage::height(Some(&image));
    if width == 0 || height == 0 {
        return None;
    }
    let size = usize::try_from(pixel_size).ok()?;
    let stride = size.checked_mul(4)?;
    let mut pixels = vec![0; stride.checked_mul(size)?];
    let colors = CGColorSpace::new_device_rgb()?;
    // SAFETY: pixels provides size rows of stride bytes and outlives the context.
    // Big-endian component order with alpha last produces RGBA8 bytes.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            size,
            size,
            8,
            stride,
            Some(&colors),
            CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0,
        )
    }?;
    let fit = f64::from(pixel_size) / width.max(height) as f64;
    let draw_width = width as f64 * fit;
    let draw_height = height as f64 * fit;
    CGContext::translate_ctm(Some(&context), 0.0, f64::from(pixel_size));
    CGContext::scale_ctm(Some(&context), 1.0, -1.0);
    CGContext::draw_image(
        Some(&context),
        NSRect::new(
            NSPoint::new(
                (f64::from(pixel_size) - draw_width) / 2.0,
                (f64::from(pixel_size) - draw_height) / 2.0,
            ),
            NSSize::new(draw_width, draw_height),
        ),
        Some(&image),
    );
    drop(context);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[..3].copy_from_slice(&[color.r, color.g, color.b]);
    }
    Some(ImageData {
        data: Blob::new(Arc::new(pixels)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: pixel_size,
        height: pixel_size,
    })
}
