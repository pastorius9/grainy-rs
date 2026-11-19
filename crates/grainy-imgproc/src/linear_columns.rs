/// The horizontal coefficients of `cv2.resize(band, (width, rows), interpolation=cv2.INTER_LINEAR)` from
/// `source` columns: source index and float32 weights per output column.
pub struct LinearColumns {
    index: Vec<usize>,
    alpha: Vec<[f32; 2]>,
    /// Columns from here on use only their first source value (the right border).
    xmax: usize,
}

impl LinearColumns {
    /// The table of `resizeGeneric_` for `source` → `width` columns.
    pub fn new(source: usize, width: usize) -> crate::Result<Self> {
        if source == 0 || width == 0 || source > i32::MAX as usize || width > i32::MAX as usize {
            return Err(crate::Error::Shape("resize dimensions must be positive OpenCV integers"));
        }
        let scale_x = 1.0 / (width as f64 / source as f64);
        let mut xmax = width;
        let mut index = Vec::with_capacity(width);
        let mut alpha = Vec::with_capacity(width);
        for dx in 0..width {
            let mut fx = ((dx as f64 + 0.5) * scale_x - 0.5) as f32;
            let mut sx = fx.floor() as isize;
            fx -= sx as f32;
            if sx < 0 {
                fx = 0.0;
                sx = 0;
            }
            if sx + 1 >= source as isize {
                xmax = xmax.min(dx);
                if sx >= source as isize - 1 {
                    fx = 0.0;
                    sx = source as isize - 1;
                }
            }
            index.push(sx as usize);
            alpha.push([1.0 - fx, fx]);
        }
        Ok(Self { index, alpha, xmax })
    }

    /// One float64 row resized (`HResizeLinear<double, double, float>`; the vertical pass of an equal height
    /// multiplies by 1 and adds the next row times 0).
    pub fn row(&self, source: &[f64], out: &mut [f64]) -> crate::Result<()> {
        if out.len() != self.index.len() {
            return Err(crate::Error::Shape("resize output row has the wrong width"));
        }
        if self
            .index
            .iter()
            .enumerate()
            .any(|(dx, &sx)| sx >= source.len() || (dx < self.xmax && sx + 1 >= source.len()))
        {
            return Err(crate::Error::Shape("resize source row too short"));
        }
        for (dx, slot) in out.iter_mut().enumerate().take(self.index.len()) {
            let sx = self.index[dx];
            let [a0, a1] = self.alpha[dx];
            *slot = if dx < self.xmax {
                source[sx] * f64::from(a0) + source[sx + 1] * f64::from(a1)
            } else {
                source[sx] * f64::from(a0)
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::LinearColumns;

    #[test]
    fn invalid_dimensions_and_rows_return_errors() {
        assert!(LinearColumns::new(0, 4).is_err());
        assert!(LinearColumns::new(4, 0).is_err());
        assert!(LinearColumns::new(usize::MAX, 4).is_err());
        let table = LinearColumns::new(3, 6).unwrap();
        assert!(table.row(&[1.0, 2.0], &mut [0.0; 6]).is_err());
        assert!(table.row(&[1.0, 2.0, 3.0], &mut [0.0; 5]).is_err());
    }
}
