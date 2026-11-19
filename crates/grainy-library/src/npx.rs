//! The few numpy / OpenCV operations `folder_recovery.compare` leans on, with numpy's float32 summation order
//! (pairwise, blocks of 128 with eight accumulators) so that threshold decisions agree bit for bit.

use ndarray::Array3;

pub(crate) use grainy_core::numpy::pairwise as sum_f32;

pub(crate) use grainy_core::numpy::mean_f32;

/// `ndarray.std()` of a float32 array.
pub fn std_f32(a: &[f32]) -> f32 {
    let mean = mean_f32(a);
    let squares: Vec<f32> = a.iter().map(|v| (*v - mean) * (*v - mean)).collect();
    (sum_f32(&squares) / a.len() as f32).sqrt()
}

/// `np.quantile(a, q)` of a float32 array (linear method), as float32.
pub fn quantile_f32(a: &[f32], q: f64) -> f32 {
    let mut sorted: Vec<f32> = a.to_vec();
    sorted.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let virtual_index = (n as f64 - 1.0) * q;
    let previous = virtual_index.floor();
    let gamma = virtual_index - previous;
    let (p, nx) = if virtual_index >= n as f64 - 1.0 {
        (n - 1, n - 1)
    } else if virtual_index <= 0.0 {
        (0, 0)
    } else {
        (previous as usize, previous as usize + 1)
    };
    let (x, y) = (f64::from(sorted[p]), f64::from(sorted[nx]));
    let diff = y - x;
    (if gamma >= 0.5 { y - diff * (1.0 - gamma) } else { x + diff * gamma }) as f32
}

/// Port of cv2.getGaussianKernel(5, sigma, CV_32F) through the shared OpenCV implementation.
pub fn gaussian_kernel5(sigma: f64) -> [f32; 5] {
    grainy_imgproc::get_gaussian_kernel(5, sigma).map(|k| std::array::from_fn(|i| k[i])).unwrap_or([f32::NAN; 5])
}

/// Port of cv2.GaussianBlur(image, (5,5), sigma) with reflect-101 borders.
pub fn gaussian_blur5(image: &Array3<f32>, sigma: f64) -> Array3<f32> {
    grainy_imgproc::gaussian_blur(image, (5, 5), sigma, 0.0, grainy_imgproc::Border::Reflect101)
        .unwrap_or_else(|_| image.clone())
}
