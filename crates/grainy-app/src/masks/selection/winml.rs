//! Optional local U²-Net inference through Windows Machine Learning. No network access is
//! needed. U2NET_HOME follows the model directory convention used by rembg.
use grainy_local::selection::SIDE;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING};
use windows::AI::MachineLearning::{LearningModel, LearningModelBinding, LearningModelSession, TensorFloat};

fn model_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GRAINY_U2NET_MODEL") {
        return Some(p.into());
    }
    let standard = std::env::var_os("U2NET_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".u2net")))?;
    ["u2net.onnx", "u2netp.onnx"].into_iter().map(|name| standard.join(name)).find(|path| path.is_file())
}

fn run(path: &Path, rgb: &[[u8; 3]]) -> windows::core::Result<Vec<f32>> {
    let model = LearningModel::LoadFromFilePath(&HSTRING::from(path.as_os_str()))?;
    let input_name = model.InputFeatures()?.GetAt(0)?.Name()?;
    let output_name = model.OutputFeatures()?.GetAt(0)?.Name()?;
    let session = LearningModelSession::CreateFromModel(&model)?;
    let binding = LearningModelBinding::CreateFromSession(&session)?;
    let max = rgb.iter().flatten().max().copied().unwrap_or(1).max(1) as f32;
    let mut input = vec![0.; 3 * SIDE * SIDE];
    for (c, (mean, sd)) in [(0.485, 0.229), (0.456, 0.224), (0.406, 0.225)].into_iter().enumerate() {
        for (i, p) in rgb.iter().enumerate() {
            input[c * SIDE * SIDE + i] = (f32::from(p[c]) / max - mean) / sd;
        }
    }
    let tensor = TensorFloat::CreateFromShapeArrayAndDataArray(&[1, 3, SIDE as i64, SIDE as i64], &input)?;
    binding.Bind(&input_name, &tensor)?;
    let output = session.Evaluate(&binding, &HSTRING::new())?;
    if !output.Succeeded()? {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(output.ErrorStatus()?)));
    }
    let mask: TensorFloat = output.Outputs()?.Lookup(&output_name)?.cast()?;
    let view = mask.GetAsVectorView()?;
    let mut values = vec![0.; SIDE * SIDE];
    if view.Size()? != values.len() as u32 || view.GetMany(0, &mut values)? != values.len() as u32 {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x80070057u32 as i32),
            "Unexpected U²-Net matte dimensions",
        ));
    }
    Ok(values)
}

pub(super) fn foreground(rgb: &[[u8; 3]]) -> Result<Option<Vec<u8>>, String> {
    let Some(path) = model_path() else {
        return Ok(None);
    };
    let mask = run(&path, rgb).map_err(|e| format!("U²-Net ({}): {e}", path.display()))?;
    let (lo, hi) = mask.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    if mask.iter().any(|v| !v.is_finite()) || hi - lo < 1e-5 {
        return Err(crate::text::tr("피사체의 경계를 찾지 못했습니다. 개체를 더 가깝게 둘러싸 주세요."));
    }
    Ok(Some(mask.into_iter().map(|v| ((v - lo) / (hi - lo) * 255.).round() as u8).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Feed the same 320×320 RGB bytes to Windows ML and an external CPU ONNX Runtime reference.
    #[test]
    #[ignore = "requires installed U²-Net and a reference RGB/matte pair"]
    fn matches_reference_inference() {
        let root = PathBuf::from(std::env::var_os("GRAINY_U2NET_REFERENCE").expect("reference directory"));
        let rgb = std::fs::read(root.join("rgb.bin")).unwrap();
        let reference = std::fs::read(root.join("model-reference.bin")).unwrap();
        let (pixels, remainder) = rgb.as_chunks::<3>();
        assert!(remainder.is_empty());
        assert_eq!(pixels.len(), SIDE * SIDE);
        let matte = foreground(pixels).unwrap().expect("local U²-Net");
        assert_eq!(matte.len(), reference.len());
        let errors = matte.iter().zip(&reference).map(|(a, b)| a.abs_diff(*b)).collect::<Vec<_>>();
        let mean = errors.iter().map(|v| f64::from(*v)).sum::<f64>() / errors.len() as f64;
        eprintln!("U²-Net reference MAE {mean:.6}, max {}", errors.iter().max().unwrap());
        assert!(mean < 0.15 && *errors.iter().max().unwrap() < 10);
    }
}
