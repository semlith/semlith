//! granite on the Neural Engine, and on the GPU through Core ML, macOS only.
//!
//! ONNX Runtime's Core ML execution provider placed almost nothing on the
//! Neural Engine (0.28.0's finding, still true). A native Core ML model does:
//! granite re-implemented in Apple's Neural Engine layout — `(B, C, 1, S)`
//! tensors, 1x1 convolutions, attention per head — and converted at fixed
//! shapes places 3 375 of 3 380 ops there and ran 236.5 chunks/s sustained on
//! the reference M1 Air, 7.7 times its CPU (2026-09-28). The models come from
//! a pack built in CI (`packs/coreml/`), pinned by digest like every other
//! component, and are compiled for this Mac on their first load.
//!
//! A model has one shape, so a batch is split into groups of the model's row
//! count and each group runs on the smallest length bucket that holds its
//! longest row. Rows are padded with a key mask of -1e4, and a padding row
//! keeps one key open so its softmax stays finite.

use anyhow::{Context, Result};
use std::path::Path;

/// What a pack says about its models: layout, rows per call, and buckets.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Manifest {
    pub pack_version: String,
    #[serde(default)]
    pub minimum_macos: Option<String>,
    pub ane: Group,
    pub gpu: Group,
}

/// One family of models in the pack.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Group {
    pub batch: usize,
    pub buckets: Vec<usize>,
    /// The file name pattern, with `{S}` for the bucket's length.
    pub file: String,
    /// For a multifunction model — one file whose buckets share one copy of
    /// the weights — the function name pattern, with `{S}` for the bucket's
    /// length. Absent where each bucket is a file of its own.
    #[serde(default)]
    pub function: Option<String>,
}

/// Which family a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NeuralEngine,
    Gpu,
}

impl Kind {
    pub fn variant(self) -> &'static str {
        match self {
            Kind::NeuralEngine => "fp16-ane",
            Kind::Gpu => "fp16-coreml-gpu",
        }
    }
}

/// The additive mask value for a padded key, as the models were converted
/// with it.
const NEG: f32 = -1e4;

pub fn manifest(pack: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(pack.join("manifest.json"))
        .with_context(|| format!("reading {}", pack.join("manifest.json").display()))?;
    serde_json::from_str(&text).context("reading the Core ML pack's manifest")
}

/// The groups of rows a batch splits into, and the bucket each runs at: rows
/// in the order given, `batch` at a time. `None` for a row longer than the
/// longest bucket.
pub fn plan(lens: &[usize], batch: usize, buckets: &[usize]) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    for (start, group) in lens.chunks(batch.max(1)).enumerate() {
        let longest = group.iter().copied().max().unwrap_or(1);
        let bucket = *buckets.iter().filter(|b| **b >= longest).min()?;
        out.push((start * batch, bucket));
    }
    Some(out)
}

/// Fill one call's inputs: ids as `i32` and the key mask as `f16` bits, both
/// `[batch, bucket]`, row-major.
pub fn inputs(rows: &[&[u32]], batch: usize, bucket: usize) -> (Vec<i32>, Vec<u16>) {
    let neg = f16_bits(NEG);
    let mut ids = vec![0i32; batch * bucket];
    let mut mask = vec![neg; batch * bucket];
    for r in 0..batch {
        match rows.get(r) {
            Some(row) => {
                for (at, id) in row.iter().take(bucket).enumerate() {
                    ids[r * bucket + at] = *id as i32;
                    mask[r * bucket + at] = 0;
                }
            }
            // A padding row: one key open, or its softmax divides by zero.
            None => mask[r * bucket] = 0,
        }
    }
    (ids, mask)
}

/// An `f32` as IEEE half-precision bits, round to nearest even. Only for the
/// two mask values, 0 and -1e4, which are exact in half precision.
fn f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x7f_ffff;
    if value == 0.0 {
        return sign;
    }
    if exp <= 0 {
        return sign;
    }
    if exp >= 31 {
        return sign | 0x7c00;
    }
    let half = (mant >> 13) as u16;
    let round = (mant >> 12) & 1;
    sign | ((exp as u16) << 10) | half.wrapping_add(round as u16)
}

#[cfg(target_os = "macos")]
pub use mac::Session;

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use anyhow::bail;
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2_core_ml::{
        MLComputeUnits, MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel,
        MLModelConfiguration, MLMultiArray, MLMultiArrayDataType,
    };
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};
    use std::path::PathBuf;

    /// One family of compiled models, loaded, and the shape they take.
    pub struct Session {
        kind: Kind,
        batch: usize,
        buckets: Vec<(usize, Retained<MLModel>)>,
        device: String,
    }

    fn error(e: Retained<objc2_foundation::NSError>) -> anyhow::Error {
        anyhow::anyhow!("{}", e.localizedDescription())
    }

    impl Session {
        /// Load every bucket of one family from an installed pack. The first
        /// load of a model on a Mac compiles it for this machine's Neural
        /// Engine (163 s for all six on the M1); later loads read that
        /// compilation back (0.8 s).
        pub fn open(
            pack: &Path,
            kind: Kind,
            progress: &mut dyn FnMut(usize, usize),
        ) -> Result<Self> {
            let manifest = manifest(pack)?;
            let group = match kind {
                Kind::NeuralEngine => &manifest.ane,
                Kind::Gpu => &manifest.gpu,
            };
            let mut buckets = Vec::new();
            for (n, bucket) in group.buckets.iter().enumerate() {
                progress(n, group.buckets.len());
                let path: PathBuf = pack.join(group.file.replace("{S}", &bucket.to_string()));
                if !path.exists() {
                    bail!("the Core ML pack has no {}", path.display());
                }
                // One configuration per bucket: in a multifunction model the
                // bucket is the function, named here, and without it every
                // bucket would load the default function's shapes.
                let config = unsafe { MLModelConfiguration::new() };
                unsafe {
                    config.setComputeUnits(match kind {
                        Kind::NeuralEngine => MLComputeUnits::CPUAndNeuralEngine,
                        Kind::Gpu => MLComputeUnits::CPUAndGPU,
                    });
                    if let Some(function) = &group.function {
                        let name = function.replace("{S}", &bucket.to_string());
                        config.setFunctionName(Some(&NSString::from_str(&name)));
                    }
                }
                let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
                // SAFETY: a file URL and a configuration this function made;
                // the call returns a retained model or an error.
                let model =
                    unsafe { MLModel::modelWithContentsOfURL_configuration_error(&url, &config) }
                        .map_err(error)
                        .with_context(|| format!("loading {}", path.display()))?;
                buckets.push((*bucket, model));
            }
            buckets.sort_by_key(|(len, _)| *len);
            Ok(Self {
                kind,
                batch: group.batch,
                buckets,
                device: match kind {
                    Kind::NeuralEngine => {
                        format!("{} Neural Engine (Core ML)", crate::system::cpu_name())
                    }
                    Kind::Gpu => format!("{} GPU (Core ML)", crate::system::cpu_name()),
                },
            })
        }

        pub fn device(&self) -> String {
            self.device.clone()
        }

        pub fn variant(&self) -> &'static str {
            self.kind.variant()
        }

        /// One batch of ids, normalised CLS vectors in the batch's order. A
        /// vector is passed through as the model gave it, zeros and NaNs
        /// included: the run checks every one and re-embeds a bad one on the
        /// CPU, which is one rule for every lane rather than one per worker.
        pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
            let lens: Vec<usize> = batch.iter().map(|ids| ids.len()).collect();
            let lengths: Vec<usize> = self.buckets.iter().map(|(len, _)| *len).collect();
            let Some(groups) = plan(&lens, self.batch, &lengths) else {
                bail!(
                    "a chunk of {} tokens is longer than the longest bucket, {}",
                    lens.iter().max().unwrap_or(&0),
                    lengths.last().unwrap_or(&0)
                );
            };
            let mut out = Vec::with_capacity(batch.len());
            for (start, bucket) in groups {
                let rows = &batch[start..(start + self.batch).min(batch.len())];
                let model = &self
                    .buckets
                    .iter()
                    .find(|(len, _)| *len == bucket)
                    .expect("a bucket the plan chose")
                    .1;
                out.extend(self.call(model, rows, bucket)?);
            }
            Ok(out)
        }

        fn call(&self, model: &MLModel, rows: &[&[u32]], bucket: usize) -> Result<Vec<Vec<f32>>> {
            let (ids, mask) = inputs(rows, self.batch, bucket);
            let shape = NSArray::from_retained_slice(&[
                NSNumber::new_usize(self.batch),
                NSNumber::new_usize(bucket),
            ]);
            // SAFETY: arrays of the shape and type the model was converted
            // with, contiguous and first-major as `initWithShape` documents;
            // each is written through its own data pointer for exactly
            // `batch * bucket` elements of its own type before it is read.
            let (ids_array, mask_array) = unsafe {
                let ids_array = MLMultiArray::initWithShape_dataType_error(
                    MLMultiArray::alloc(),
                    &shape,
                    MLMultiArrayDataType::Int32,
                )
                .map_err(error)?;
                let mask_array = MLMultiArray::initWithShape_dataType_error(
                    MLMultiArray::alloc(),
                    &shape,
                    MLMultiArrayDataType::Float16,
                )
                .map_err(error)?;
                #[allow(deprecated)]
                let into = ids_array.dataPointer().as_ptr().cast::<i32>();
                std::ptr::copy_nonoverlapping(ids.as_ptr(), into, ids.len());
                #[allow(deprecated)]
                let into = mask_array.dataPointer().as_ptr().cast::<u16>();
                std::ptr::copy_nonoverlapping(mask.as_ptr(), into, mask.len());
                (ids_array, mask_array)
            };
            // SAFETY: two feature values from arrays made above, under the
            // input names the models were converted with.
            let provider = unsafe {
                let ids_value = MLFeatureValue::featureValueWithMultiArray(&ids_array);
                let mask_value = MLFeatureValue::featureValueWithMultiArray(&mask_array);
                let keys = [NSString::from_str("ids"), NSString::from_str("kmask")];
                let values: [&AnyObject; 2] = [ids_value.as_ref(), mask_value.as_ref()];
                let dict = NSDictionary::from_slices(&[&*keys[0], &*keys[1]], &values);
                MLDictionaryFeatureProvider::initWithDictionary_error(
                    MLDictionaryFeatureProvider::alloc(),
                    &dict,
                )
                .map_err(error)?
            };
            // SAFETY: a provider this function made; the call returns the
            // output features or an error.
            let output =
                unsafe { model.predictionFromFeatures_error(ProtocolObject::from_ref(&*provider)) }
                    .map_err(error)
                    .context("running the model")?;
            // SAFETY: `cls` is the output the models were converted with, an
            // fp32 array of `[batch, 384]`; its strides are read rather than
            // assumed, and nothing past `count` is read.
            let vectors = unsafe {
                let value = output
                    .featureValueForName(&NSString::from_str("cls"))
                    .context("the model gave no `cls` output")?;
                let array = value
                    .multiArrayValue()
                    .context("the `cls` output is not an array")?;
                if array.dataType() != MLMultiArrayDataType::Float32 {
                    bail!("the `cls` output is not fp32");
                }
                let shape: Vec<usize> = array.shape().iter().map(|n| n.as_usize()).collect();
                let strides: Vec<usize> = array.strides().iter().map(|n| n.as_usize()).collect();
                if shape.len() != 2 || shape[0] != self.batch {
                    bail!("the `cls` output has shape {shape:?}");
                }
                let dim = shape[1];
                #[allow(deprecated)]
                let data = array.dataPointer().as_ptr().cast::<f32>();
                let mut vectors = Vec::with_capacity(rows.len());
                for r in 0..rows.len() {
                    let mut vector = Vec::with_capacity(dim);
                    for c in 0..dim {
                        vector.push(*data.add(r * strides[0] + c * strides[1]));
                    }
                    crate::normalize(&mut vector);
                    vectors.push(vector);
                }
                vectors
            };
            Ok(vectors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_splits_into_groups_on_the_smallest_bucket_that_holds_them() {
        let buckets = [128, 192, 256, 320, 384, 512];
        assert_eq!(
            plan(&[10, 130, 5, 7, 200, 200], 4, &buckets),
            Some(vec![(0, 192), (4, 256)])
        );
        assert_eq!(plan(&[600], 4, &buckets), None);
    }

    #[test]
    fn a_manifest_reads_with_and_without_multifunction_models() {
        let split: Manifest = serde_json::from_str(
            r#"{"pack_version":"1","ane":{"batch":4,"buckets":[128,512],"file":"ane/ane_b4_s{S}.mlmodelc"},
                "gpu":{"batch":8,"buckets":[128],"file":"gpu/std_b8_s{S}.mlmodelc"},"extra":true}"#,
        )
        .unwrap();
        assert_eq!(split.ane.function, None);
        let merged: Manifest = serde_json::from_str(
            r#"{"pack_version":"1","ane":{"batch":4,"buckets":[128],"file":"ane/ane_b4.mlmodelc","function":"s{S}"},
                "gpu":{"batch":8,"buckets":[128],"file":"gpu/std_b8.mlmodelc","function":"s{S}"}}"#,
        )
        .unwrap();
        assert_eq!(merged.ane.function.as_deref(), Some("s{S}"));
    }

    #[test]
    fn padding_rows_keep_one_key_open_and_masks_are_half_precision() {
        let (ids, mask) = inputs(&[&[7, 8]], 2, 3);
        assert_eq!(ids, vec![7, 8, 0, 0, 0, 0]);
        let neg = f16_bits(NEG);
        assert_eq!(mask, vec![0, 0, neg, 0, neg, neg]);
        // -1e4 in half precision is 0xF0E2 (-10000.0 exactly representable
        // to within half's spacing of 8 at that magnitude).
        assert_eq!(neg, 0xf0e2);
    }
}
