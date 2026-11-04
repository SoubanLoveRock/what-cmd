//! 向量存储：`vectors.bin` 的读写与 L2 归一化。
//!
//! 行优先、`f32` 小端序、行长 `dimensions`。写入前已归一化，查询时点积即余弦相似度。
//! 两个文件的原子替换由 `index::save_atomic` 统一完成，本模块只写 `.tmp`。

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// 行优先存储的向量矩阵。
#[derive(Debug, Clone, PartialEq)]
pub struct Vectors {
    /// 每行的分量数。
    pub dimensions: usize,
    /// 所有行拼接，长度应为 `rows() * dimensions`。
    pub data: Vec<f32>,
}

impl Vectors {
    /// 行数。`dimensions` 为 0 时返回 0。
    pub fn rows(&self) -> usize {
        if self.dimensions == 0 {
            0
        } else {
            self.data.len() / self.dimensions
        }
    }

    /// 第 `i` 行。越界时 panic，调用方应先用 `rows()` 检查。
    pub fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dimensions..(i + 1) * self.dimensions]
    }
}

/// L2 归一化，原地修改。零向量返回 `ZeroVector`。
pub fn normalize_vector(v: &mut [f32]) -> Result<(), VectorError> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return Err(VectorError::ZeroVector);
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    Ok(())
}

/// 点积。两者都已归一化时即余弦相似度。长度不同时按较短者计算。
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 读取 `vectors.bin`，校验文件字节数等于 `rows × dimensions × 4`。
pub fn read(path: &Path, dimensions: usize, rows: usize) -> Result<Vectors, VectorError> {
    let bytes = fs::read(path)?;
    parse(&bytes, dimensions, rows)
}

/// 解析 `vectors.bin` 的字节内容，校验长度等于 `rows × dimensions × 4`。
///
/// `index::load` 用它解析已读入并做过哈希校验的字节，避免重复读取文件。
pub fn parse(bytes: &[u8], dimensions: usize, rows: usize) -> Result<Vectors, VectorError> {
    let expected = rows as u64 * dimensions as u64 * 4;
    if bytes.len() as u64 != expected {
        return Err(VectorError::Length {
            expected,
            actual: bytes.len() as u64,
        });
    }
    let data = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(Vectors { dimensions, data })
}

/// 写入 `tmp_path(path)`，即 `path` 加 `.tmp` 后缀，不 rename。
///
/// 写完后 `sync_all`。替换正式文件由调用方（`index::save_atomic`）完成。
pub fn write_tmp(path: &Path, vectors: &Vectors) -> Result<(), VectorError> {
    let file = File::create(tmp_path(path))?;
    let mut writer = BufWriter::new(file);
    for value in &vectors.data {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok(())
}

/// 临时文件路径：在原文件名后追加 `.tmp`。
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// 向量相关错误。
#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    /// 文件读写失败。
    #[error("vector io: {0}")]
    Io(#[from] std::io::Error),
    /// 零向量无法归一化。
    #[error("zero vector cannot be normalized")]
    ZeroVector,
    /// 维度不一致。
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    Dimension { expected: usize, actual: usize },
    /// 文件字节数与 `rows × dimensions × 4` 不符。
    #[error("vectors.bin length {actual} does not match {expected} bytes")]
    Length { expected: u64, actual: u64 },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{VectorError, Vectors, dot, normalize_vector, read, tmp_path, write_tmp};

    #[test]
    fn normalized_dot_equals_cosine() {
        // cos([3,4], [4,3]) = 24 / 25 = 0.96
        let mut a = [3.0_f32, 4.0];
        let mut b = [4.0_f32, 3.0];
        normalize_vector(&mut a).unwrap();
        normalize_vector(&mut b).unwrap();
        assert!((dot(&a, &b) - 0.96).abs() < 1e-6);
    }

    #[test]
    fn normalized_vector_has_unit_length() {
        let mut v = [1.0_f32, 2.0, 2.0];
        normalize_vector(&mut v).unwrap();
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_vector_is_rejected() {
        let mut v = [0.0_f32, 0.0];
        assert!(matches!(
            normalize_vector(&mut v),
            Err(VectorError::ZeroVector)
        ));
    }

    #[test]
    fn orthogonal_vectors_have_zero_dot() {
        assert_eq!(dot(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn rows_and_row_follow_dimensions() {
        let v = Vectors {
            dimensions: 2,
            data: vec![1.0, 2.0, 3.0, 4.0],
        };
        assert_eq!(v.rows(), 2);
        assert_eq!(v.row(1), &[3.0, 4.0]);
    }

    #[test]
    fn write_tmp_then_read_round_trips() {
        let dir = crate::testutil::tempdir();
        let path = dir.path().join("vectors.bin");
        let original = Vectors {
            dimensions: 3,
            data: vec![0.5, -1.25, 2.0, 1.0, 0.0, -0.75],
        };
        write_tmp(&path, &original).unwrap();
        // 只写 .tmp，正式文件由 save_atomic 替换。
        assert!(!path.exists());
        fs::rename(tmp_path(&path), &path).unwrap();
        let back = read(&path, 3, 2).unwrap();
        assert_eq!(back, original);
    }

    #[test]
    fn read_rejects_wrong_length() {
        let dir = crate::testutil::tempdir();
        let path = dir.path().join("vectors.bin");
        let v = Vectors {
            dimensions: 2,
            data: vec![1.0, 2.0, 3.0, 4.0],
        };
        write_tmp(&path, &v).unwrap();
        fs::rename(tmp_path(&path), &path).unwrap();
        // 文件含 2 行，声明 3 行时必须拒绝。
        assert!(matches!(
            read(&path, 2, 3),
            Err(VectorError::Length {
                expected: 24,
                actual: 16
            })
        ));
    }

    #[test]
    fn tmp_path_appends_suffix() {
        let p = std::path::Path::new("/x/vectors.bin");
        assert_eq!(tmp_path(p), std::path::Path::new("/x/vectors.bin.tmp"));
    }
}
