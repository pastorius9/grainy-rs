//! Rec.2100 PQ photo delivery. The engine supplies linear sRGB or linear ProPhoto,
//! with diffuse white at 1.0. The 10-bit AV1 stream and its AVIF container both signal CICP 9/16/9.

use avif_serialize::{constants, Aviffy};
use grainy_core::Image;
use rav1e::prelude::*;

use crate::{codec, invalid, Result};

// RGB-to-XYZ primary matrices, followed by XYZ-to-Rec.2020. ProPhoto uses Bradford D50→D65.
const SRGB_TO_2020: [[f64; 3]; 3] =
    [[0.6274039, 0.3292830, 0.0433131], [0.0690973, 0.9195404, 0.0113623], [0.0163914, 0.0880133, 0.8955953]];
const PROPHOTO_TO_2020: [[f64; 3]; 3] =
    [[1.2006507, -0.0575637, -0.1430870], [-0.0699434, 1.0806279, -0.0106845], [0.0055415, -0.0407826, 1.0352411]];

/// Inverse ST 2084 EOTF, with ITU-R BT.2408's 203 cd/m² reference white.
fn pq(linear: f64) -> f64 {
    let luminance = (linear * (203.0 / 10_000.0)).clamp(0.0, 1.0);
    let y = luminance.powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * y) / (1.0 + 2392.0 / 128.0 * y)).powf(2523.0 / 32.0)
}

fn fill(frame: &mut Frame<u16>, image: &Image, matrix: &[[f64; 3]; 3]) {
    let (height, width, _) = image.dim();
    let [luma, chroma_b, chroma_r] = &mut frame.planes;
    let (ys, us, vs) = (luma.cfg.stride, chroma_b.cfg.stride, chroma_r.cfg.stride);
    let (yp, up, vp) = (luma.data_origin_mut(), chroma_b.data_origin_mut(), chroma_r.data_origin_mut());
    for y0 in (0..height).step_by(2) {
        for x0 in (0..width).step_by(2) {
            let (mut cb, mut cr, mut n) = (0.0, 0.0, 0.0);
            for y in y0..(y0 + 2).min(height) {
                for x in x0..(x0 + 2).min(width) {
                    let rgb = [0, 1, 2].map(|c| f64::from(image[[y, x, c]]));
                    let [r, g, b] = matrix.map(|m| pq(m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2]));
                    let signal = 0.2627 * r + 0.6780 * g + 0.0593 * b;
                    yp[y * ys + x] = (64.0 + 876.0 * signal).round() as u16;
                    cb += (b - signal) / (2.0 * (1.0 - 0.0593));
                    cr += (r - signal) / (2.0 * (1.0 - 0.2627));
                    n += 1.0;
                }
            }
            let (x, y) = (x0 / 2, y0 / 2);
            up[y * us + x] = (512.0 + 896.0 * cb / n).round() as u16;
            vp[y * vs + x] = (512.0 + 896.0 * cr / n).round() as u16;
        }
    }
}

pub(crate) fn encode(image: &Image, wide: bool, quality: f64) -> Result<Vec<u8>> {
    let (height, width, _) = image.dim();
    let encoder = EncoderConfig {
        height,
        width,
        bit_depth: 10,
        chroma_sampling: ChromaSampling::Cs420,
        pixel_range: PixelRange::Limited,
        color_description: Some(ColorDescription {
            color_primaries: ColorPrimaries::BT2020,
            transfer_characteristics: TransferCharacteristics::SMPTE2084,
            matrix_coefficients: MatrixCoefficients::BT2020NCL,
        }),
        still_picture: true,
        low_latency: true,
        quantizer: ((100.0 - quality) * 2.55).round() as usize,
        ..EncoderConfig::with_speed_preset(6)
    };
    let mut ctx: Context<u16> = Config::new().with_encoder_config(encoder).new_context().map_err(codec)?;
    let mut frame = ctx.new_frame();
    fill(&mut frame, image, if wide { &PROPHOTO_TO_2020 } else { &SRGB_TO_2020 });
    ctx.send_frame(frame).map_err(codec)?;
    ctx.flush();
    let packet = loop {
        match ctx.receive_packet() {
            Ok(packet) => break packet,
            Err(EncoderStatus::Encoded) => continue,
            Err(EncoderStatus::LimitReached) => return Err(invalid("HDR AV1 encoder returned no image")),
            Err(error) => return Err(codec(error)),
        }
    };
    let mut out = Vec::new();
    Aviffy::new()
        .set_chroma_subsampling((true, true))
        .set_seq_profile(0)
        .set_full_color_range(false)
        .set_color_primaries(constants::ColorPrimaries::Bt2020)
        .set_transfer_characteristics(constants::TransferCharacteristics::Smpte2084)
        .set_matrix_coefficients(constants::MatrixCoefficients::Bt2020Ncl)
        .write(&mut out, &packet.data, None, width as u32, height as u32, 10)?;
    Ok(out)
}
