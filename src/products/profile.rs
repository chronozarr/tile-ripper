//! Declarative scientific product descriptions and closed quality policies.
//!
//! A profile describes expected source metadata. It does not install a reader,
//! authorize a remote service, or qualify any object for acquisition.

use crate::{fail, Result};
use serde::Serialize;

pub const LANDSAT_C2_L2_SR: &str = "landsat-c2-l2-sr";
pub const LANDSAT_TM_C2_L2_SR: &str = "landsat-tm-c2-l2-sr";
pub const LANDSAT_ETM_C2_L2_SR: &str = "landsat-etm-c2-l2-sr";
pub const HLS_L30_V2: &str = "hls-l30";
pub const HLS_S30_V2: &str = "hls-s30";
pub const OPERA_DSWX_HLS_V1: &str = "opera-dswx-hls";
pub const OPERA_DSWX_S1_V1: &str = "opera-dswx-s1";
pub const OPERA_DIST_ALERT_V1: &str = "opera-dist-alert";
pub const OPERA_RTC_S1_V1: &str = "opera-rtc-s1";
/// Products served by the native HDF5 array readers. Listed outside the
/// `native-hdf5` gate so other builds can name the missing feature.
pub const ARRAY_PRODUCTS: &[&str] = &[
    "goes19-abi-cmipc-c13",
    "goes19-abi-cmipc-c01",
    "goes19-abi-cmipm1-c02",
    "smap-spl3smp-e-v6-am",
    "smap-spl3smp-e-v6-pm",
    "smap-spl3smp-e-v6-north-am",
    "smap-spl3smp-e-v6-north-pm",
    "nisar-gcov-p1-fa-hhhh",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductFamily {
    Landsat,
    Hls,
    OperaDswx,
    OperaRtc,
    OperaDswxS1,
    OperaDist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DataType {
    #[serde(rename = "uint8")]
    UInt8,
    #[serde(rename = "uint16")]
    UInt16,
    #[serde(rename = "int16")]
    Int16,
    #[serde(rename = "float32")]
    Float32,
}

impl DataType {
    pub const fn name(self) -> &'static str {
        match self {
            Self::UInt8 => "uint8",
            Self::UInt16 => "uint16",
            Self::Int16 => "int16",
            Self::Float32 => "float32",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BandRole {
    SurfaceReflectance,
    TopOfAtmosphereReflectance,
    BrightnessTemperature,
    Quality,
    Angle,
    Classification,
    Diagnostic,
    Elevation,
    Backscatter,
    Confidence,
    Date,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum NoDataValue {
    #[serde(rename = "uint8")]
    UInt8(u8),
    #[serde(rename = "uint16")]
    UInt16(u16),
    #[serde(rename = "int16")]
    Int16(i16),
}

impl NoDataValue {
    pub const fn as_f64(self) -> f64 {
        match self {
            Self::UInt8(value) => value as f64,
            Self::UInt16(value) => value as f64,
            Self::Int16(value) => value as f64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BandProfile {
    /// Product-native layer name.
    pub name: String,
    /// Exact key expected in the product's named catalog/STAC representation.
    pub asset_key: String,
    /// Accepted user-facing names. Aliases never change source semantics.
    pub aliases: &'static [&'static str],
    pub role: BandRole,
    /// Scientific meaning of the stored layer; calibration remains separate.
    pub description: &'static str,
    pub dtype: DataType,
    pub nodata: Option<NoDataValue>,
    pub scale: Option<f64>,
    pub offset: Option<f64>,
    pub unit: &'static str,
    /// True when the product may legitimately omit this layer.
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeGrid {
    pub posting_m: u16,
    pub projection: &'static str,
    pub organization: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CatalogIdentity {
    pub provider: &'static str,
    pub catalog: &'static str,
    pub collection: &'static str,
    pub version: &'static str,
    pub access: &'static str,
    pub platforms: &'static [&'static str],
    pub processing_levels: &'static [&'static str],
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProductProfile {
    pub id: &'static str,
    pub family: ProductFamily,
    pub mission: &'static str,
    pub instrument: &'static str,
    pub title: &'static str,
    /// Preferred processing release. Each source retains its exact release.
    pub version: &'static str,
    pub processing_versions: &'static [&'static str],
    pub native_grid: NativeGrid,
    pub catalog: CatalogIdentity,
    pub bands: Vec<BandProfile>,
    pub quality_policies: Vec<QualityPolicy>,
    pub capability_caveat: &'static str,
}

impl ProductProfile {
    /// Resolve the source release, independently of a catalog's collection
    /// version (which can remain unchanged across processing releases).
    pub fn processing_version(&self, granule: &str) -> Result<&'static str> {
        self.validate_identity(granule, None, None)?;
        if self.family == ProductFamily::OperaDswx {
            match granule.split('_').nth(8) {
                Some("v1.0") => Ok("1.0"),
                Some("v1.1") => Ok("1.1"),
                _ => fail("unsupported DSWx processing version"),
            }
        } else {
            Ok(self.version)
        }
    }
    /// Resolve a canonical layer name, catalog asset key, or documented alias.
    pub fn band(&self, name: &str) -> Result<&BandProfile> {
        let matches = self
            .bands
            .iter()
            .filter(|band| {
                eq_name(&band.name, name)
                    || eq_name(&band.asset_key, name)
                    || band.aliases.iter().any(|alias| eq_name(alias, name))
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [band] => Ok(*band),
            [] => fail("unknown band or alias for product profile"),
            _ => fail("ambiguous band alias for product profile"),
        }
    }

    pub fn quality_policy(&self, name: &str) -> Result<QualityPolicy> {
        self.quality_policies
            .iter()
            .copied()
            .find(|policy| eq_name(policy.name(), name))
            .ok_or_else(|| "unknown quality policy for product profile".into())
    }

    /// Validate product identity fields before any physical-format admission.
    ///
    /// This validates the narrow product/release identity only. It is not a
    /// substitute for inspecting the selected asset, grid, dtype, or metadata.
    pub fn validate_identity(
        &self,
        granule: &str,
        platform: Option<&str>,
        processing: Option<&str>,
    ) -> Result<()> {
        if granule.is_empty() {
            return fail("product identity requires a granule name");
        }
        if let Some(value) = platform {
            if !self
                .catalog
                .platforms
                .iter()
                .any(|allowed| eq_identity(allowed, value))
            {
                return fail("platform does not belong to product profile");
            }
        }
        if let Some(value) = processing {
            if !self
                .catalog
                .processing_levels
                .iter()
                .any(|allowed| eq_identity(allowed, value))
            {
                return fail("processing level does not belong to product profile");
            }
        }

        match self.family {
            ProductFamily::Landsat => {
                let identity = parse_landsat_identity(granule)?;
                let allowed = match self.id {
                    LANDSAT_TM_C2_L2_SR => matches!(identity.platform, "LANDSAT_4" | "LANDSAT_5"),
                    LANDSAT_ETM_C2_L2_SR => identity.platform == "LANDSAT_7",
                    _ => matches!(identity.platform, "LANDSAT_8" | "LANDSAT_9"),
                };
                if !allowed {
                    return fail("Landsat sensor does not belong to this profile");
                }
                validate_landsat_identity(granule, platform, processing)
            }
            ProductFamily::Hls => {
                let kind = if self.id == HLS_L30_V2 { "L30" } else { "S30" };
                parse_hls_identity(granule, kind).map(|_| ())
            }
            ProductFamily::OperaDswx => {
                let identity = parse_opera_identity(granule, false)?;
                validate_parsed_platform(identity.platform, platform)
            }
            ProductFamily::OperaDswxS1 | ProductFamily::OperaDist => {
                let identity =
                    parse_wave2_opera_identity(granule, self.family == ProductFamily::OperaDist)?;
                validate_parsed_platform(identity.platform, platform)
            }
            ProductFamily::OperaRtc => {
                let identity = parse_opera_identity(granule, true)?;
                validate_parsed_platform(identity.platform, platform)
            }
        }
    }

    /// Bind a catalog timestamp to the acquisition token in the granule name.
    /// HLS identifiers carry a source-scene nominal token, so HLS binds the
    /// calendar day; Landsat binds its acquisition date and OPERA binds seconds.
    pub fn validate_datetime(&self, granule: &str, datetime: &str) -> Result<()> {
        let observed = chrono::DateTime::parse_from_rfc3339(datetime)?.naive_utc();
        let expected = match self.family {
            ProductFamily::Landsat => parse_landsat_identity(granule)?.acquisition,
            ProductFamily::Hls => {
                let kind = if self.id == HLS_L30_V2 { "L30" } else { "S30" };
                parse_hls_identity(granule, kind)?
            }
            ProductFamily::OperaDswx => parse_opera_identity(granule, false)?.acquisition,
            ProductFamily::OperaRtc => parse_opera_identity(granule, true)?.acquisition,
            ProductFamily::OperaDswxS1 | ProductFamily::OperaDist => {
                parse_wave2_opera_identity(granule, self.family == ProductFamily::OperaDist)?
                    .acquisition
            }
        };
        let matches = match self.family {
            ProductFamily::Landsat
            | ProductFamily::Hls
            | ProductFamily::OperaDist
            | ProductFamily::OperaDswxS1 => observed.date() == expected.date(),
            // OPERA filename timestamps have whole-second resolution. Retain
            // the catalog's subsecond precision without rejecting that same
            // acquisition or accepting an adjacent second.
            ProductFamily::OperaDswx | ProductFamily::OperaRtc => {
                observed.and_utc().timestamp() == expected.and_utc().timestamp()
            }
        };
        if matches {
            Ok(())
        } else {
            fail("observation datetime contradicts granule acquisition identity")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum QualitySample {
    UInt8(u8),
    UInt16(u16),
    Int16(i16),
    Float32(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPolicy {
    LandsatTmEtmPixelClear,
    LandsatTmRadiometricallyUnsaturated,
    LandsatEtmRadiometricallyUnsaturated,
    LandsatTmEtmCloudClear,
    LandsatPixelClear,
    LandsatRadiometricallyUnsaturated,
    LandsatAerosolValid,
    LandsatAerosolRecommended,
    HlsFmaskClear,
    HlsFmaskClearLowAerosol,
    DswxS1WtrWater,
    DswxS1BwtrWater,
    DistVegConfirmed,
    DistGenConfirmed,
    DistVegConfirmedOngoing,
    DistGenConfirmedOngoing,
    DistCurrentLand,
    DistCurrentAssessed,
    DswxWtrWater,
    DswxBwtrWater,
    DswxConfUnmaskedWater,
    DswxConfAnyWater,
    DswxWtr1Water,
    DswxWtr2Water,
    DswxShadowFree,
    DswxCloudObstructionFree,
    OperaRtcFinite,
    OperaRtcMaskValid,
    OperaRtcMaskUnaffected,
}

impl QualityPolicy {
    pub const fn name(self) -> &'static str {
        match self {
            Self::LandsatTmEtmPixelClear => "landsat_tm_etm_pixel_clear",
            Self::LandsatTmRadiometricallyUnsaturated => "landsat_tm_radiometrically_unsaturated",
            Self::LandsatEtmRadiometricallyUnsaturated => "landsat_etm_radiometrically_unsaturated",
            Self::LandsatTmEtmCloudClear => "landsat_tm_etm_cloud_clear",
            Self::LandsatPixelClear => "landsat_pixel_clear",
            Self::LandsatRadiometricallyUnsaturated => "landsat_radiometrically_unsaturated",
            Self::LandsatAerosolValid => "landsat_aerosol_valid",
            Self::LandsatAerosolRecommended => "landsat_aerosol_recommended",
            Self::HlsFmaskClear => "hls_fmask_clear",
            Self::HlsFmaskClearLowAerosol => "hls_fmask_clear_low_aerosol",
            Self::DswxS1WtrWater => "dswx_s1_wtr_water",
            Self::DswxS1BwtrWater => "dswx_s1_bwtr_water",
            Self::DistVegConfirmed => "dist_veg_confirmed",
            Self::DistGenConfirmed => "dist_gen_confirmed",
            Self::DistVegConfirmedOngoing => "dist_veg_confirmed_ongoing",
            Self::DistGenConfirmedOngoing => "dist_gen_confirmed_ongoing",
            Self::DistCurrentLand => "dist_current_land",
            Self::DistCurrentAssessed => "dist_current_assessed",
            Self::DswxWtrWater => "dswx_wtr_water",
            Self::DswxBwtrWater => "dswx_bwtr_water",
            Self::DswxConfUnmaskedWater => "dswx_conf_unmasked_water",
            Self::DswxConfAnyWater => "dswx_conf_any_water",
            Self::DswxWtr1Water => "dswx_wtr1_water",
            Self::DswxWtr2Water => "dswx_wtr2_water",
            Self::DswxShadowFree => "dswx_shadow_free",
            Self::DswxCloudObstructionFree => "dswx_cloud_obstruction_free",
            Self::OperaRtcFinite => "opera_rtc_finite",
            Self::OperaRtcMaskValid => "opera_rtc_mask_valid",
            Self::OperaRtcMaskUnaffected => "opera_rtc_mask_unaffected",
        }
    }

    /// Raw layers to which this policy may be applied. Multiple values are
    /// alternatives for polarization-dependent RTC imagery, not a band stack.
    pub const fn required_bands(self) -> &'static [&'static str] {
        match self {
            Self::LandsatTmEtmPixelClear => &["QA_PIXEL"],
            Self::LandsatTmRadiometricallyUnsaturated
            | Self::LandsatEtmRadiometricallyUnsaturated => &["QA_RADSAT"],
            Self::LandsatTmEtmCloudClear => &["SR_CLOUD_QA"],
            Self::LandsatPixelClear => &["QA_PIXEL"],
            Self::LandsatRadiometricallyUnsaturated => &["QA_RADSAT"],
            Self::LandsatAerosolValid | Self::LandsatAerosolRecommended => &["SR_QA_AEROSOL"],
            Self::HlsFmaskClear | Self::HlsFmaskClearLowAerosol => &["Fmask"],
            Self::DswxS1WtrWater => &["WTR"],
            Self::DswxS1BwtrWater => &["BWTR"],
            Self::DistVegConfirmed | Self::DistVegConfirmedOngoing => &["VEG-DIST-STATUS"],
            Self::DistGenConfirmed | Self::DistGenConfirmedOngoing => &["GEN-DIST-STATUS"],
            Self::DistCurrentLand | Self::DistCurrentAssessed => &["DATA-MASK"],
            Self::DswxWtrWater => &["WTR"],
            Self::DswxBwtrWater => &["BWTR"],
            Self::DswxConfUnmaskedWater | Self::DswxConfAnyWater => &["CONF"],
            Self::DswxWtr1Water => &["WTR-1"],
            Self::DswxWtr2Water => &["WTR-2"],
            Self::DswxShadowFree => &["SHAD"],
            Self::DswxCloudObstructionFree => &["CLOUD"],
            Self::OperaRtcFinite => &["VV", "VH", "HH", "HV"],
            Self::OperaRtcMaskValid | Self::OperaRtcMaskUnaffected => &["mask"],
        }
    }

    /// Compatibility helper for policies tied to one exact raw QA/class layer.
    /// Use `required_bands` for the polarization-dependent RTC finite policy.
    pub const fn required_band(self) -> &'static str {
        self.required_bands()[0]
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::LandsatTmEtmPixelClear => "TM/ETM+ QA_PIXEL fill, dilated cloud, cloud, cloud shadow and snow bits absent; clear bit 6 is not used because of the documented Landsat 4-7 water issue",
            Self::LandsatTmRadiometricallyUnsaturated => "TM QA_RADSAT all defined saturation and dropped-pixel bits zero (mask 0x027f)",
            Self::LandsatEtmRadiometricallyUnsaturated => "ETM+ QA_RADSAT all defined saturation including band 6H and dropped-pixel bits zero (mask 0x037f); scan gaps are never filled",
            Self::LandsatTmEtmCloudClear => "LEDAPS SR_CLOUD_QA cloud, shadow, adjacent-cloud and snow bits absent; DDV and water bits retained",
            Self::LandsatPixelClear => {
                "QA_PIXEL is internally defined and fill, dilated cloud, cirrus, cloud, cloud shadow, and snow are all absent"
            }
            Self::LandsatRadiometricallyUnsaturated => {
                "all defined QA_RADSAT saturation and terrain-occlusion bits are zero"
            }
            Self::LandsatAerosolValid => {
                "SR_QA_AEROSOL is one of the documented valid center-pixel retrieval codes"
            }
            Self::LandsatAerosolRecommended => {
                "valid center-pixel aerosol retrieval with climatology, low, or medium aerosol; high aerosol is excluded per the product guide"
            }
            Self::HlsFmaskClear => {
                "Fmask is not fill and cloud, adjacent cloud/shadow, cloud shadow, and snow/ice bits are zero"
            }
            Self::HlsFmaskClearLowAerosol => {
                "HLS clear policy plus climatology or low aerosol"
            }
            Self::DswxS1WtrWater => "DSWx-S1 WTR open water (1) or inundated vegetation (3); HLS partial-water class 2 is not reused",
            Self::DswxS1BwtrWater => "DSWx-S1 BWTR water class 1; HAND, layover/shadow and ocean excluded",
            Self::DistVegConfirmed | Self::DistGenConfirmed => "DIST accumulated confirmed disturbance status classes 3,6,7,8, including finished alerts; not an observation-quality mask",
            Self::DistVegConfirmedOngoing | Self::DistGenConfirmedOngoing => "DIST accumulated confirmed ongoing disturbance status classes 3 or 6; no history inferred from a single alert",
            Self::DistCurrentLand => "current input HLS DATA-MASK land class 1; independent of accumulated alert state",
            Self::DistCurrentAssessed => "current input HLS DATA-MASK land or water classes 1,2; independent of accumulated alert state",
            Self::DswxWtrWater => "WTR class is open water or partial surface water",
            Self::DswxBwtrWater => "BWTR class is water",
            Self::DswxConfUnmaskedWater => {
                "CONF is an unmasked open- or partial-water confidence class"
            }
            Self::DswxConfAnyWater => {
                "CONF is an open- or partial-water class, including cloud/shadow- or snow/ice-obstructed classes"
            }
            Self::DswxWtr1Water => "WTR-1 class is open or partial surface water",
            Self::DswxWtr2Water => "WTR-2 class is open or partial surface water",
            Self::DswxShadowFree => "SHAD class is not shadow (class 1)",
            Self::DswxCloudObstructionFree => {
                "CLOUD low three obstruction bits are zero; bit 3 is retained without exclusion and its interpretation follows the source processor metadata; ocean mask and fill are excluded"
            }
            Self::OperaRtcFinite => "raw linear gamma0 backscatter sample is finite",
            Self::OperaRtcMaskValid => "RTC mask is a valid sample (classes 0 through 3)",
            Self::OperaRtcMaskUnaffected => {
                "RTC mask is valid and unaffected by layover or shadow (class 0)"
            }
        }
    }

    /// Evaluate one raw source sample without altering or calibrating it.
    pub fn accepts(self, sample: QualitySample) -> Result<bool> {
        match (self, sample) {
            (Self::LandsatTmEtmPixelClear, QualitySample::UInt16(word)) => {
                if word & 0xc004 != 0 || ((word >> 10) & 3) == 2 || ((word >> 12) & 3) == 2 {
                    return fail("TM/ETM+ QA_PIXEL reserved bits set");
                }
                Ok(word & 0x003b == 0)
            }
            (
                Self::LandsatTmRadiometricallyUnsaturated
                | Self::LandsatEtmRadiometricallyUnsaturated,
                QualitySample::UInt16(word),
            ) => {
                let allowed = if self == Self::LandsatTmRadiometricallyUnsaturated {
                    0x027f
                } else {
                    0x037f
                };
                if word & !allowed != 0 {
                    return fail("TM/ETM+ QA_RADSAT reserved bits set");
                }
                Ok(word == 0)
            }
            (Self::LandsatTmEtmCloudClear, QualitySample::UInt8(word)) => {
                if ![0, 1, 2, 4, 8, 9, 12, 16, 20, 24, 32, 34, 36, 40, 48, 52, 56].contains(&word) {
                    return fail("undocumented LEDAPS SR_CLOUD_QA code");
                }
                Ok(word != 0 && word & 0x1e == 0)
            }
            (Self::LandsatPixelClear, QualitySample::UInt16(word)) => {
                validate_landsat_pixel(word)?;
                Ok(word & 0x003f == 0 && word & (1 << 6) != 0)
            }
            (Self::LandsatRadiometricallyUnsaturated, QualitySample::UInt16(word)) => {
                if word & !0x097f != 0 {
                    return fail("QA_RADSAT contains an unused bit");
                }
                Ok(word == 0)
            }
            (Self::LandsatAerosolValid, QualitySample::UInt16(word)) => {
                validate_landsat_aerosol(word)?;
                Ok(matches!(word, 2 | 66 | 130 | 194))
            }
            (Self::LandsatAerosolValid, QualitySample::UInt8(word)) => {
                let word = u16::from(word);
                validate_landsat_aerosol(word)?;
                Ok(matches!(word, 2 | 66 | 130 | 194))
            }
            (Self::LandsatAerosolRecommended, QualitySample::UInt16(word)) => {
                validate_landsat_aerosol(word)?;
                Ok(matches!(word, 2 | 66 | 130))
            }
            (Self::LandsatAerosolRecommended, QualitySample::UInt8(word)) => {
                let word = u16::from(word);
                validate_landsat_aerosol(word)?;
                Ok(matches!(word, 2 | 66 | 130))
            }
            (Self::HlsFmaskClear, QualitySample::UInt8(word)) => {
                validate_hls_fmask(word)?;
                Ok(word & 0x1e == 0)
            }
            (Self::HlsFmaskClearLowAerosol, QualitySample::UInt8(word)) => {
                validate_hls_fmask(word)?;
                Ok(word & 0x1e == 0 && ((word >> 6) & 0x03) <= 1)
            }
            (Self::DswxS1WtrWater, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 3, 250, 251, 254, 255], "DSWx-S1 WTR")?;
                Ok(matches!(class, 1 | 3))
            }
            (Self::DswxS1BwtrWater, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 250, 251, 254, 255], "DSWx-S1 BWTR")?;
                Ok(class == 1)
            }
            (
                Self::DistVegConfirmed
                | Self::DistGenConfirmed
                | Self::DistVegConfirmedOngoing
                | Self::DistGenConfirmedOngoing,
                QualitySample::UInt8(class),
            ) => {
                if class > 8 && class != 255 {
                    return fail("undefined DIST status class");
                }
                Ok(
                    if matches!(
                        self,
                        Self::DistVegConfirmedOngoing | Self::DistGenConfirmedOngoing
                    ) {
                        matches!(class, 3 | 6)
                    } else {
                        matches!(class, 3 | 6 | 7 | 8)
                    },
                )
            }
            (Self::DistCurrentLand | Self::DistCurrentAssessed, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 2], "DIST DATA-MASK")?;
                Ok(class == 1 || self == Self::DistCurrentAssessed && class == 2)
            }
            (Self::DswxWtrWater, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 2, 252, 253, 254, 255], "WTR")?;
                Ok(matches!(class, 1 | 2))
            }
            (Self::DswxBwtrWater, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 252, 253, 254, 255], "BWTR")?;
                Ok(class == 1)
            }
            (Self::DswxConfUnmaskedWater, QualitySample::UInt8(class)) => {
                validate_dswx_conf(class)?;
                Ok((1..=4).contains(&class))
            }
            (Self::DswxConfAnyWater, QualitySample::UInt8(class)) => {
                validate_dswx_conf(class)?;
                Ok(matches!(class, 1..=4 | 11..=14 | 21..=24))
            }
            (Self::DswxWtr1Water | Self::DswxWtr2Water, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 2, 254, 255], "WTR intermediate")?;
                Ok(matches!(class, 1 | 2))
            }
            (Self::DswxShadowFree, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1], "SHAD")?;
                Ok(class == 1)
            }
            (Self::DswxCloudObstructionFree, QualitySample::UInt8(class)) => {
                if !(class <= 15 || matches!(class, 254 | 255)) {
                    return fail("undefined DSWx CLOUD class");
                }
                Ok(matches!(class, 0 | 8))
            }
            (Self::OperaRtcFinite, QualitySample::Float32(value)) => Ok(value.is_finite()),
            (Self::OperaRtcMaskValid, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 2, 3, 255], "RTC mask")?;
                Ok(class <= 3)
            }
            (Self::OperaRtcMaskUnaffected, QualitySample::UInt8(class)) => {
                validate_class(class, &[0, 1, 2, 3, 255], "RTC mask")?;
                Ok(class == 0)
            }
            _ => fail("quality policy received the wrong raw sample type"),
        }
    }
}

pub fn profile(id: &str) -> Result<ProductProfile> {
    let id = match id.to_ascii_lowercase().as_str() {
        "hls-l30-v2" => HLS_L30_V2,
        "hls-s30-v2" => HLS_S30_V2,
        "opera-dswx-hls-v1" => OPERA_DSWX_HLS_V1,
        "opera-rtc-s1-v1" => OPERA_RTC_S1_V1,
        "opera-dswx-s1-v1" => OPERA_DSWX_S1_V1,
        "opera-dist-alert-v1" | "opera-dist-alert-hls" => OPERA_DIST_ALERT_V1,
        _ => id,
    };
    profiles()
        .into_iter()
        .find(|candidate| eq_name(candidate.id, id))
        .ok_or_else(|| {
            if cfg!(not(feature = "native-hdf5")) && ARRAY_PRODUCTS.contains(&id) {
                format!("{id} needs the native-hdf5 Cargo feature, which this build does not include; rebuild with --no-default-features --features native-cog,native-hdf5,authenticated-http (see docs/installation.md)").into()
            } else {
                "unknown scientific product profile".into()
            }
        })
}

pub fn profiles() -> Vec<ProductProfile> {
    vec![
        landsat(),
        historical_landsat(false),
        historical_landsat(true),
        hls_l30(),
        hls_s30(),
        dswx(),
        rtc(),
        dswx_s1(),
        dist_alert(),
    ]
}

impl std::str::FromStr for QualityPolicy {
    type Err = Box<dyn std::error::Error + Send + Sync>;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        profiles()
            .into_iter()
            .flat_map(|profile| profile.quality_policies)
            .find(|policy| eq_name(policy.name(), value))
            .ok_or_else(|| "unknown quality policy".into())
    }
}

#[allow(clippy::too_many_arguments)]
fn band(
    name: &'static str,
    aliases: &'static [&'static str],
    role: BandRole,
    data_type: DataType,
    nodata: Option<NoDataValue>,
    scale: f64,
    offset: f64,
    unit: &'static str,
) -> BandProfile {
    BandProfile {
        name: name.to_owned(),
        asset_key: name.to_owned(),
        aliases,
        role,
        description: match role {
            BandRole::SurfaceReflectance => "native stored surface-reflectance code",
            BandRole::TopOfAtmosphereReflectance => {
                "native stored top-of-atmosphere reflectance code"
            }
            BandRole::BrightnessTemperature => "native stored brightness-temperature code",
            BandRole::Quality => "native quality or condition code",
            BandRole::Angle => "native stored observation angle",
            BandRole::Classification => "native categorical classification",
            BandRole::Diagnostic => {
                "native five-digit decimal encoding of five binary diagnostic tests"
            }
            BandRole::Elevation => "native elevation in metres above the WGS84 ellipsoid",
            BandRole::Backscatter => "native linear gamma0 backscatter sample",
            BandRole::Confidence => "native algorithm confidence code, not a probability",
            BandRole::Date => "native date code with its source-defined epoch",
        },
        dtype: data_type,
        nodata,
        scale: Some(scale),
        offset: Some(offset),
        unit,
        optional: false,
    }
}

fn asset_key(mut band: BandProfile, key: &'static str) -> BandProfile {
    band.asset_key = key.to_owned();
    band
}

fn landsat() -> ProductProfile {
    let sr = |name, aliases| {
        band(
            name,
            aliases,
            BandRole::SurfaceReflectance,
            DataType::UInt16,
            Some(NoDataValue::UInt16(0)),
            0.0000275,
            -0.2,
            "1",
        )
    };
    ProductProfile {
        id: LANDSAT_C2_L2_SR,
        family: ProductFamily::Landsat,
        mission: "Landsat 8/9",
        instrument: "Operational Land Imager (OLI/OLI-2)",
        title: "Landsat 8/9 Collection 2 Level-2 surface reflectance",
        version: "Collection 2",
        processing_versions: &["Collection 2"],
        native_grid: NativeGrid {
            posting_m: 30,
            projection:
                "scene-native UTM (polar stereographic where specified by the Landsat product)",
            organization: "WRS-2 scene",
        },
        catalog: CatalogIdentity {
            provider: "Microsoft Planetary Computer (USGS Landsat Collection 2 mirror)",
            catalog: "Planetary Computer STAC",
            collection: "landsat-c2-l2",
            version: "Collection 2",
            access: "runtime-signed Azure HTTPS COG assets from the Planetary Computer STAC item",
            platforms: &["LANDSAT_8", "LANDSAT_9"],
            processing_levels: &["L2SP", "L2SR"],
        },
        bands: vec![
            asset_key(sr("SR_B1", &["coastal", "coastal_aerosol"]), "coastal"),
            asset_key(sr("SR_B2", &["blue"]), "blue"),
            asset_key(sr("SR_B3", &["green"]), "green"),
            asset_key(sr("SR_B4", &["red"]), "red"),
            asset_key(sr("SR_B5", &["nir", "nir_narrow"]), "nir08"),
            asset_key(sr("SR_B6", &["swir1", "swir_1"]), "swir16"),
            asset_key(sr("SR_B7", &["swir2", "swir_2"]), "swir22"),
            asset_key(
                band(
                    "QA_PIXEL",
                    &["pixel_qa"],
                    BandRole::Quality,
                    DataType::UInt16,
                    Some(NoDataValue::UInt16(1)),
                    1.0,
                    0.0,
                    "1",
                ),
                "qa_pixel",
            ),
            asset_key(
                band(
                    "QA_RADSAT",
                    &["radsat_qa"],
                    BandRole::Quality,
                    DataType::UInt16,
                    None,
                    1.0,
                    0.0,
                    "1",
                ),
                "qa_radsat",
            ),
            asset_key(
                band(
                    "SR_QA_AEROSOL",
                    &["aerosol_qa"],
                    BandRole::Quality,
                    DataType::UInt8,
                    Some(NoDataValue::UInt8(1)),
                    1.0,
                    0.0,
                    "1",
                ),
                "qa_aerosol",
            ),
        ],
        quality_policies: vec![
            QualityPolicy::LandsatPixelClear,
            QualityPolicy::LandsatRadiometricallyUnsaturated,
            QualityPolicy::LandsatAerosolValid,
            QualityPolicy::LandsatAerosolRecommended,
        ],
        capability_caveat: capability_caveat(),
    }
}

fn historical_landsat(etm: bool) -> ProductProfile {
    let mut p = landsat();
    p.id = if etm {
        LANDSAT_ETM_C2_L2_SR
    } else {
        LANDSAT_TM_C2_L2_SR
    };
    p.mission = if etm { "Landsat 7" } else { "Landsat 4/5" };
    p.instrument = if etm {
        "Enhanced Thematic Mapper Plus (ETM+)"
    } else {
        "Thematic Mapper (TM)"
    };
    p.title = if etm {
        "Landsat 7 ETM+ Collection 2 Level-2 surface reflectance"
    } else {
        "Landsat 4/5 TM Collection 2 Level-2 surface reflectance"
    };
    p.catalog.platforms = if etm {
        &["LANDSAT_7"]
    } else {
        &["LANDSAT_4", "LANDSAT_5"]
    };
    let sr = |name, aliases, key| {
        asset_key(
            band(
                name,
                aliases,
                BandRole::SurfaceReflectance,
                DataType::UInt16,
                Some(NoDataValue::UInt16(0)),
                0.0000275,
                -0.2,
                "1",
            ),
            key,
        )
    };
    let qa = |name, aliases, key| {
        asset_key(
            band(
                name,
                aliases,
                BandRole::Quality,
                DataType::UInt8,
                Some(NoDataValue::UInt8(0)),
                1.,
                0.,
                "1",
            ),
            key,
        )
    };
    let pixel = p.band("QA_PIXEL").expect("Landsat pixel QA").clone();
    let radsat = p.band("QA_RADSAT").expect("Landsat saturation QA").clone();
    let mut opacity = asset_key(
        band(
            "SR_ATMOS_OPACITY",
            &["atmos_opacity"],
            BandRole::Quality,
            DataType::Int16,
            Some(NoDataValue::Int16(-9999)),
            0.001,
            0.,
            "1",
        ),
        "atmos_opacity",
    );
    opacity.description =
        "LEDAPS atmospheric opacity code, advisory rather than probability; stored scale 0.001";
    p.bands = vec![
        sr("SR_B1", &["blue"], "blue"),
        sr("SR_B2", &["green"], "green"),
        sr("SR_B3", &["red"], "red"),
        sr("SR_B4", &["nir", "nir08"], "nir08"),
        sr("SR_B5", &["swir1", "swir16"], "swir16"),
        sr("SR_B7", &["swir2", "swir22"], "swir22"),
        pixel,
        radsat,
        qa("SR_CLOUD_QA", &["cloud_qa"], "cloud_qa"),
        opacity,
    ];
    p.quality_policies = vec![
        QualityPolicy::LandsatTmEtmPixelClear,
        if etm {
            QualityPolicy::LandsatEtmRadiometricallyUnsaturated
        } else {
            QualityPolicy::LandsatTmRadiometricallyUnsaturated
        },
        QualityPolicy::LandsatTmEtmCloudClear,
    ];
    p
}

fn hls_l30() -> ProductProfile {
    let mut bands = vec![
        hls_reflectance("B01", &["coastal", "coastal_aerosol"]),
        hls_reflectance("B02", &["blue"]),
        hls_reflectance("B03", &["green"]),
        hls_reflectance("B04", &["red"]),
        hls_reflectance("B05", &["nir", "nir_narrow"]),
        hls_reflectance("B06", &["swir1", "swir_1"]),
        hls_reflectance("B07", &["swir2", "swir_2"]),
        hls_toa("B09", &["cirrus"]),
        hls_temperature("B10", &["thermal1", "tir1"]),
        hls_temperature("B11", &["thermal2", "tir2"]),
    ];
    bands.extend(hls_common_bands());
    hls_profile(
        HLS_L30_V2,
        "Harmonized Landsat Sentinel-2 L30 V2.0",
        "HLSL30_2.0",
        &["LANDSAT_8", "LANDSAT_9"],
        "L30",
        bands,
    )
}

fn hls_s30() -> ProductProfile {
    let mut bands = vec![
        hls_reflectance("B01", &["coastal", "coastal_aerosol"]),
        hls_reflectance("B02", &["blue"]),
        hls_reflectance("B03", &["green"]),
        hls_reflectance("B04", &["red"]),
        hls_reflectance("B05", &["red_edge_1"]),
        hls_reflectance("B06", &["red_edge_2"]),
        hls_reflectance("B07", &["red_edge_3"]),
        hls_reflectance("B08", &["nir_broad"]),
        hls_reflectance("B8A", &["nir", "nir_narrow"]),
        hls_toa("B09", &["water_vapor"]),
        hls_toa("B10", &["cirrus"]),
        hls_reflectance("B11", &["swir1", "swir_1"]),
        hls_reflectance("B12", &["swir2", "swir_2"]),
    ];
    bands.extend(hls_common_bands());
    hls_profile(
        HLS_S30_V2,
        "Harmonized Landsat Sentinel-2 S30 V2.0",
        "HLSS30_2.0",
        &["SENTINEL_2A", "SENTINEL_2B", "SENTINEL_2C"],
        "S30",
        bands,
    )
}

fn hls_profile(
    id: &'static str,
    title: &'static str,
    collection: &'static str,
    platforms: &'static [&'static str],
    processing: &'static str,
    bands: Vec<BandProfile>,
) -> ProductProfile {
    ProductProfile {
        id,
        family: ProductFamily::Hls,
        mission: "Harmonized Landsat Sentinel-2",
        instrument: if processing == "L30" {
            "Landsat 8/9 OLI and TIRS"
        } else {
            "Sentinel-2A/B/C MSI"
        },
        title,
        version: "2.0",
        processing_versions: &["2.0"],
        native_grid: NativeGrid {
            posting_m: 30,
            projection: "UTM",
            organization: "MGRS-aligned 109.8 km tile",
        },
        catalog: CatalogIdentity {
            provider: "NASA LP DAAC",
            catalog: "NASA CMR STAC LPCLOUD",
            collection,
            version: "2.0",
            access: "Earthdata-authenticated HTTPS or temporary in-region S3 credentials",
            platforms,
            processing_levels: if processing == "L30" {
                &["L30"]
            } else {
                &["S30"]
            },
        },
        bands,
        quality_policies: vec![
            QualityPolicy::HlsFmaskClear,
            QualityPolicy::HlsFmaskClearLowAerosol,
        ],
        capability_caveat: capability_caveat(),
    }
}

fn hls_reflectance(name: &'static str, aliases: &'static [&'static str]) -> BandProfile {
    band(
        name,
        aliases,
        BandRole::SurfaceReflectance,
        DataType::Int16,
        Some(NoDataValue::Int16(-9999)),
        0.0001,
        0.0,
        "1",
    )
}

fn hls_toa(name: &'static str, aliases: &'static [&'static str]) -> BandProfile {
    let mut value = hls_reflectance(name, aliases);
    value.role = BandRole::TopOfAtmosphereReflectance;
    value.description = "native stored top-of-atmosphere reflectance code";
    value
}

fn hls_temperature(name: &'static str, aliases: &'static [&'static str]) -> BandProfile {
    band(
        name,
        aliases,
        BandRole::BrightnessTemperature,
        DataType::Int16,
        Some(NoDataValue::Int16(-9999)),
        0.01,
        0.0,
        "degree_Celsius",
    )
}

fn hls_common_bands() -> Vec<BandProfile> {
    let angle = |name, aliases| {
        band(
            name,
            aliases,
            BandRole::Angle,
            DataType::UInt16,
            Some(NoDataValue::UInt16(40_000)),
            0.01,
            0.0,
            "degree",
        )
    };
    vec![
        band(
            "Fmask",
            &["fmask", "qa"],
            BandRole::Quality,
            DataType::UInt8,
            Some(NoDataValue::UInt8(255)),
            1.0,
            0.0,
            "1",
        ),
        angle("SZA", &["sun_zenith"]),
        angle("SAA", &["sun_azimuth"]),
        angle("VZA", &["view_zenith"]),
        angle("VAA", &["view_azimuth"]),
    ]
}

fn dswx() -> ProductProfile {
    let class = |name, aliases, dtype, nodata| {
        band(
            name,
            aliases,
            BandRole::Classification,
            dtype,
            nodata,
            1.0,
            0.0,
            "1",
        )
    };
    ProductProfile {
        id: OPERA_DSWX_HLS_V1,
        family: ProductFamily::OperaDswx,
        mission: "NASA OPERA",
        instrument: "HLS source OLI or MSI, identified per observation",
        title: "OPERA Level-3 Dynamic Surface Water Extent from HLS V1",
        version: "1.1",
        processing_versions: &["1.0", "1.1"],
        native_grid: NativeGrid {
            posting_m: 30,
            projection: "UTM",
            organization: "HLS MGRS tile, 3660 by 3660 pixels",
        },
        catalog: CatalogIdentity {
            provider: "NASA PO.DAAC",
            catalog: "NASA CMR STAC POCloud",
            collection: "OPERA_L3_DSWX-HLS_V1",
            version: "1.0",
            access: "Earthdata-authenticated PO.DAAC HTTPS or temporary in-region S3 credentials",
            platforms: &[
                "LANDSAT_8",
                "LANDSAT_9",
                "SENTINEL_2A",
                "SENTINEL_2B",
                "SENTINEL_2C",
            ],
            processing_levels: &["L3", "DSWX-HLS"],
        },
        bands: vec![
            class(
                "WTR",
                &["water_classification"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            class(
                "BWTR",
                &["binary_water"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            class(
                "CONF",
                &["confidence"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            band(
                "DIAG",
                &["diagnostic"],
                BandRole::Diagnostic,
                DataType::UInt16,
                Some(NoDataValue::UInt16(65_535)),
                1.0,
                0.0,
                "1",
            ),
            class(
                "WTR-1",
                &["wtr1"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            class(
                "WTR-2",
                &["wtr2"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            class(
                "LAND",
                &["land_cover"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            class("SHAD", &["terrain_shadow"], DataType::UInt8, None),
            class(
                "CLOUD",
                &["hls_cloud_snow_fmask"],
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
            ),
            band(
                "DEM",
                &["elevation"],
                BandRole::Elevation,
                DataType::Float32,
                None,
                1.0,
                0.0,
                "m",
            ),
        ],
        quality_policies: vec![
            QualityPolicy::DswxWtrWater,
            QualityPolicy::DswxBwtrWater,
            QualityPolicy::DswxConfUnmaskedWater,
            QualityPolicy::DswxConfAnyWater,
            QualityPolicy::DswxWtr1Water,
            QualityPolicy::DswxWtr2Water,
            QualityPolicy::DswxShadowFree,
            QualityPolicy::DswxCloudObstructionFree,
        ],
        capability_caveat: capability_caveat(),
    }
}

fn rtc() -> ProductProfile {
    let polarization = |name, aliases| {
        let mut value = band(
            name,
            aliases,
            BandRole::Backscatter,
            DataType::Float32,
            None,
            1.0,
            0.0,
            "1",
        );
        value.optional = true;
        value
    };
    ProductProfile {
        id: OPERA_RTC_S1_V1,
        family: ProductFamily::OperaRtc,
        mission: "NASA OPERA Sentinel-1",
        instrument: "Sentinel-1 C-SAR",
        title: "OPERA Level-2 RTC SAR backscatter from Sentinel-1 V1",
        version: "1.0",
        processing_versions: &["1.0"],
        native_grid: NativeGrid {
            posting_m: 30,
            projection:
                "UTM; EPSG:3413 polar stereographic for products centered north of 75 degrees",
            organization: "predefined Sentinel-1 burst grid",
        },
        catalog: CatalogIdentity {
            provider: "NASA ASF DAAC",
            catalog: "NASA CMR/ASF Vertex",
            collection: "OPERA_L2_RTC-S1_V1",
            version: "1",
            access: "Earthdata-authenticated ASF HTTPS or temporary in-region S3 credentials",
            platforms: &["SENTINEL_1A", "SENTINEL_1B", "SENTINEL_1C"],
            processing_levels: &["L2", "RTC-S1"],
        },
        bands: vec![
            polarization("VV", &["vv"]),
            polarization("VH", &["vh"]),
            polarization("HH", &["hh"]),
            polarization("HV", &["hv"]),
            band(
                "mask",
                &["layover_shadow_mask"],
                BandRole::Quality,
                DataType::UInt8,
                Some(NoDataValue::UInt8(255)),
                1.0,
                0.0,
                "1",
            ),
        ],
        quality_policies: vec![
            QualityPolicy::OperaRtcFinite,
            QualityPolicy::OperaRtcMaskValid,
            QualityPolicy::OperaRtcMaskUnaffected,
        ],
        capability_caveat: capability_caveat(),
    }
}

fn dswx_s1() -> ProductProfile {
    let mut p = dswx();
    p.id = OPERA_DSWX_S1_V1;
    p.family = ProductFamily::OperaDswxS1;
    p.mission = "NASA OPERA Sentinel-1";
    p.instrument = "Sentinel-1 C-SAR RTC-S1 mosaics";
    p.title = "OPERA Level-3 Dynamic Surface Water Extent from Sentinel-1 V1.0";
    p.version = "1.0";
    p.processing_versions = &["1.0"];
    p.catalog.collection = "OPERA_L3_DSWX-S1_V1";
    p.catalog.version = "1.0";
    p.catalog.processing_levels = &["L3", "DSWx-S1"];
    p.catalog.platforms = &["SENTINEL_1A", "SENTINEL_1B", "SENTINEL_1C", "SENTINEL_1D"];
    p.bands = [("WTR", 255, BandRole::Classification), ("BWTR", 255, BandRole::Classification),
        ("CONF", 255, BandRole::Confidence), ("DIAG", 255, BandRole::Diagnostic)].into_iter().map(|(name, fill, role)| {
        let mut b = band(name, &[], role, DataType::UInt8, Some(NoDataValue::UInt8(fill)), 1., 0., "1");
        b.description = match name { "WTR" => "DSWx-S1 water classes: 0 not water, 1 open water, 3 inundated vegetation, 250 HAND, 251 layover/shadow, 254 ocean, 255 fill",
            "BWTR" => "DSWx-S1 binary water and separate HAND/layover-shadow/ocean masking codes",
            "CONF" => "DSWx-S1 categorical confidence codes 0/1/2/5/6/7, wetland30/31/32/35/36/37, HAND250/layover251/ocean254/fill255; not probability or HLS CONF",
            _ => "DSWx-S1 fuzzy score 0..100 with HAND250, layover251, ocean254, fill255; distinct from HLS diagnostics" };
        b
    }).collect();
    p.quality_policies = vec![
        QualityPolicy::DswxS1WtrWater,
        QualityPolicy::DswxS1BwtrWater,
    ];
    p
}

fn dist_alert() -> ProductProfile {
    let mut p = dswx();
    p.id = OPERA_DIST_ALERT_V1;
    p.family = ProductFamily::OperaDist;
    p.mission = "NASA OPERA HLS disturbance";
    p.instrument = "HLS L30/S30 input observations";
    p.title = "OPERA Level-3 DIST-ALERT-HLS V1 accumulated alert observation";
    p.version = "1";
    p.processing_versions = &["1"];
    p.catalog.provider = "NASA LP DAAC";
    p.catalog.catalog = "NASA CMR LPCLOUD";
    p.catalog.collection = "OPERA_L3_DIST-ALERT-HLS_V1";
    p.catalog.version = "1";
    p.catalog.processing_levels = &["L3", "DIST-ALERT-HLS"];
    p.catalog.access = "explicit Earthdata-authenticated LP DAAC HTTPS COG assets";
    p.catalog.platforms = &[
        "LANDSAT_8",
        "LANDSAT_9",
        "SENTINEL_2A",
        "SENTINEL_2B",
        "SENTINEL_2C",
    ];
    p.bands = ["VEG-DIST-STATUS", "GEN-DIST-STATUS", "VEG-DIST-CONF", "GEN-DIST-CONF", "VEG-DIST-DATE", "GEN-DIST-DATE", "VEG-LAST-DATE", "GEN-LAST-DATE", "DATA-MASK"].into_iter().map(|name| {
        let status = name.ends_with("STATUS");
        let mask = name == "DATA-MASK";
        let mut b = band(name, &[], if status {BandRole::Classification} else if mask {BandRole::Quality} else if name.ends_with("CONF") {BandRole::Confidence} else {BandRole::Date},
            if status || mask {DataType::UInt8} else {DataType::Int16},
            if mask {None} else {Some(if status {NoDataValue::UInt8(255)} else {NoDataValue::Int16(-1)})}, 1., 0., "1");
        b.description = if status { "Accumulated status: 0 no disturbance; 1/4 first, 2/5 provisional, 3/6 confirmed ongoing, 7/8 confirmed finished; VEG pairs denote below/at-least50% loss, GEN pairs low/high spectral anomaly; 255 missing" }
            else if mask { "Current HLS scene observation mask: 0 not assessed land, 1 land, 2 water; no source fill; does not mask accumulated status" }
            else if name.ends_with("CONF") { "Accumulated algorithm confidence score, not a probability; -1 missing" }
            else if name.ends_with("LAST-DATE") { "Most recent quality-assessed observation with sufficient baseline, days since 2020-12-31; -1 missing; GEN land, VEG land/water" }
            else { "Stored onset date: days since 2020-12-31; -1 missing, 0 no anomaly; retain native Int16 and epoch" };
        if name.ends_with("DATE") { b.unit = "days since 2020-12-31"; }
        b
    }).collect();
    p.quality_policies = vec![
        QualityPolicy::DistVegConfirmed,
        QualityPolicy::DistGenConfirmed,
        QualityPolicy::DistVegConfirmedOngoing,
        QualityPolicy::DistGenConfirmedOngoing,
        QualityPolicy::DistCurrentLand,
        QualityPolicy::DistCurrentAssessed,
    ];
    p
}

fn parse_wave2_opera_identity(granule: &str, dist: bool) -> Result<ParsedIdentity> {
    let parts = granule.split('_').collect::<Vec<_>>();
    if parts.len() < 9
        || parts[..3]
            != [
                "OPERA",
                "L3",
                if dist { "DIST-ALERT-HLS" } else { "DSWx-S1" },
            ]
        || !valid_mgrs_tile(parts[3])
        || parts[7] != "30"
        || parts[8] != if dist { "v1" } else { "v1.0" }
        || parse_timestamp(parts[5]).is_err()
    {
        return fail("granule is not the exact Wave 2 OPERA product/version");
    }
    let p = if dist { dist_alert() } else { dswx_s1() };
    if parts.len() > 9 {
        let mut suffix = parts[9..].join("_");
        if suffix.ends_with(".tif") {
            suffix.truncate(suffix.len() - 4);
        }
        if !dist {
            let number = match suffix.as_str() {
                "B01_WTR" => "WTR",
                "B02_BWTR" => "BWTR",
                "B03_CONF" => "CONF",
                "B04_DIAG" => "DIAG",
                _ => return fail("invalid DSWx-S1 layer suffix"),
            };
            p.band(number)?;
        } else {
            p.band(&suffix)?;
        }
    }
    let platform = match (dist, parts[6]) {
        (true, "L8") => "LANDSAT_8",
        (true, "L9") => "LANDSAT_9",
        (true, "S2A") => "SENTINEL_2A",
        (true, "S2B") => "SENTINEL_2B",
        (true, "S2C") => "SENTINEL_2C",
        (false, "S1A") => "SENTINEL_1A",
        (false, "S1B") => "SENTINEL_1B",
        (false, "S1C") => "SENTINEL_1C",
        (false, "S1D") => "SENTINEL_1D",
        _ => return fail("Wave 2 OPERA platform does not belong to product"),
    };
    Ok(ParsedIdentity {
        acquisition: parse_timestamp(parts[4])?,
        platform,
    })
}

const fn capability_caveat() -> &'static str {
    "Declarative metadata only: this profile grants no acquisition, authentication, decoding, alignment, calibration, harmonization, or live-product qualification capability."
}

fn validate_landsat_pixel(word: u16) -> Result<()> {
    let cloud = (word >> 8) & 0x03;
    let shadow = (word >> 10) & 0x03;
    let snow = (word >> 12) & 0x03;
    let cirrus = (word >> 14) & 0x03;
    if shadow == 2 || snow == 2 || cirrus == 2 {
        return fail("QA_PIXEL contains a reserved confidence state");
    }
    if word & 1 != 0 {
        return if word == 1 {
            Ok(())
        } else {
            fail("QA_PIXEL fill code contains additional undefined fields")
        };
    }
    let clear = word & (1 << 6) != 0;
    let expected_clear = word & ((1 << 1) | (1 << 3)) == 0;
    if clear != expected_clear
        || (word & (1 << 3) != 0) != (cloud == 3)
        || (word & (1 << 4) != 0) != (shadow == 3)
        || (word & (1 << 5) != 0) != (snow == 3)
        || (word & (1 << 2) != 0) != (cirrus == 3)
    {
        return fail("QA_PIXEL flag and confidence fields are contradictory");
    }
    Ok(())
}

fn validate_landsat_aerosol(word: u16) -> Result<()> {
    const DOCUMENTED: &[u16] = &[
        1, 2, 4, 32, 66, 68, 96, 100, 130, 132, 160, 164, 192, 194, 196, 224, 228,
    ];
    if !DOCUMENTED.contains(&word) {
        return fail("undefined Landsat 8/9 SR_QA_AEROSOL code");
    }
    Ok(())
}

fn validate_hls_fmask(word: u8) -> Result<()> {
    if word == 255 {
        return Ok(());
    }
    if word & 1 != 0 {
        return fail("HLS V2 Fmask reserved cirrus bit is set");
    }
    Ok(())
}

fn validate_dswx_conf(class: u8) -> Result<()> {
    if matches!(class, 0..=4 | 10..=14 | 20..=24 | 254 | 255) {
        Ok(())
    } else {
        fail("undefined DSWx CONF class")
    }
}

fn validate_class(class: u8, allowed: &[u8], layer: &str) -> Result<()> {
    if allowed.contains(&class) {
        Ok(())
    } else {
        fail(&format!("undefined {layer} class"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedIdentity {
    acquisition: chrono::NaiveDateTime,
    platform: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LandsatIdentity {
    acquisition: chrono::NaiveDateTime,
    platform: &'static str,
    processing: String,
    path_row: String,
    collection: String,
    tier: String,
}

fn parse_timestamp(value: &str) -> Result<chrono::NaiveDateTime> {
    Ok(chrono::NaiveDateTime::parse_from_str(
        value,
        "%Y%m%dT%H%M%SZ",
    )?)
}

fn valid_mgrs_tile(value: &str) -> bool {
    value.len() == 6
        && value.starts_with('T')
        && value[1..].bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn parse_hls_identity(granule: &str, kind: &str) -> Result<chrono::NaiveDateTime> {
    let parts = granule.split('.').collect::<Vec<_>>();
    if parts.len() != 6
        || parts[0] != "HLS"
        || parts[1] != kind
        || !valid_mgrs_tile(parts[2])
        || !parts[4].eq_ignore_ascii_case("v2")
        || parts[5] != "0"
    {
        return fail("granule is not the expected HLS V2 product");
    }
    chrono::NaiveDateTime::parse_from_str(parts[3], "%Y%jT%H%M%S")
        .map_err(|_| "HLS granule has an invalid acquisition token".into())
}

fn opera_suffix_valid(parts: &[&str], rtc: bool) -> bool {
    if parts.is_empty() {
        return true;
    }
    let mut suffix = parts.join("_");
    if suffix.to_ascii_lowercase().ends_with(".tif") {
        suffix.truncate(suffix.len() - 4);
    }
    if !rtc
        && suffix.len() > 4
        && suffix.as_bytes()[0] == b'B'
        && suffix.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
        && suffix.as_bytes()[3] == b'_'
    {
        suffix = suffix[4..].to_owned();
    }
    if rtc {
        ["VV", "VH", "HH", "HV", "mask"]
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&suffix))
    } else {
        [
            "WTR", "BWTR", "CONF", "DIAG", "WTR-1", "WTR-2", "LAND", "SHAD", "CLOUD", "DEM",
        ]
        .iter()
        .any(|value| value.eq_ignore_ascii_case(&suffix))
    }
}

fn valid_rtc_burst(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[0].starts_with('T')
        && parts[0][1..].bytes().all(|byte| byte.is_ascii_digit())
        && parts[1].len() == 6
        && parts[1].bytes().all(|byte| byte.is_ascii_digit())
        && matches!(parts[2], "IW1" | "IW2" | "IW3")
}

fn parse_opera_identity(granule: &str, rtc: bool) -> Result<ParsedIdentity> {
    let parts = granule.split('_').collect::<Vec<_>>();
    let expected = if rtc {
        ["OPERA", "L2", "RTC-S1"]
    } else {
        ["OPERA", "L3", "DSWx-HLS"]
    };
    let platform = parts.get(6).copied().unwrap_or_default();
    let platform = match (rtc, platform) {
        (true, "S1A") => "SENTINEL_1A",
        (true, "S1B") => "SENTINEL_1B",
        (true, "S1C") => "SENTINEL_1C",
        (false, "L8") => "LANDSAT_8",
        (false, "L9") => "LANDSAT_9",
        (false, "S2A") => "SENTINEL_2A",
        (false, "S2B") => "SENTINEL_2B",
        (false, "S2C") => "SENTINEL_2C",
        _ => return fail("OPERA granule has an invalid platform token"),
    };
    if parts.len() < 9
        || parts[..3] != expected
        || if rtc {
            !valid_rtc_burst(parts[3])
        } else {
            !valid_mgrs_tile(parts[3])
        }
        || parse_timestamp(parts[4]).is_err()
        || parse_timestamp(parts[5]).is_err()
        || parts[7] != "30"
        || if rtc {
            parts[8] != "v1.0"
        } else {
            !matches!(parts[8], "v1.0" | "v1.1")
        }
        || !opera_suffix_valid(&parts[9..], rtc)
    {
        return fail(if rtc {
            "granule is not an OPERA RTC-S1 V1 30 m product"
        } else {
            "granule is not an OPERA DSWx-HLS V1 30 m product"
        });
    }
    Ok(ParsedIdentity {
        acquisition: parse_timestamp(parts[4])?,
        platform,
    })
}

fn landsat_suffix_valid(parts: &[&str]) -> bool {
    if parts.is_empty() {
        return true;
    }
    let mut suffix = parts.join("_");
    if suffix.to_ascii_lowercase().ends_with(".tif") {
        suffix.truncate(suffix.len() - 4);
    }
    matches!(
        suffix.as_str(),
        "SR_B1"
            | "SR_B2"
            | "SR_B3"
            | "SR_B4"
            | "SR_B5"
            | "SR_B6"
            | "SR_B7"
            | "QA_PIXEL"
            | "QA_RADSAT"
            | "SR_CLOUD_QA"
            | "SR_ATMOS_OPACITY"
            | "SR_QA_AEROSOL"
    )
}

fn parse_landsat_identity(granule: &str) -> Result<LandsatIdentity> {
    let parts = granule.split('_').collect::<Vec<_>>();
    if parts.len() < 6 || !matches!(parts[0], "LT04" | "LT05" | "LE07" | "LC08" | "LC09") {
        return fail("granule is not a Landsat 8/9 Collection 2 Level-2 SR product");
    }
    let platform = match parts[0] {
        "LT04" => "LANDSAT_4",
        "LT05" => "LANDSAT_5",
        "LE07" => "LANDSAT_7",
        "LC08" => "LANDSAT_8",
        _ => "LANDSAT_9",
    };
    let processing = parts[1];
    let path_row = parts[2];
    let acquisition = chrono::NaiveDate::parse_from_str(parts[3], "%Y%m%d")?
        .and_hms_opt(0, 0, 0)
        .ok_or("invalid Landsat acquisition date")?;
    let has_processing_date = parts
        .get(4)
        .is_some_and(|value| value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()));
    let collection_index = if has_processing_date { 5 } else { 4 };
    let tier_index = collection_index + 1;
    let suffix_index = tier_index + 1;
    if !matches!(processing, "L2SP" | "L2SR")
        || path_row.len() != 6
        || !path_row.bytes().all(|byte| byte.is_ascii_digit())
        || has_processing_date && chrono::NaiveDate::parse_from_str(parts[4], "%Y%m%d").is_err()
        || parts.get(collection_index) != Some(&"02")
        || !matches!(parts.get(tier_index), Some(&("T1" | "T2" | "RT")))
        || !landsat_suffix_valid(parts.get(suffix_index..).unwrap_or_default())
    {
        return fail("granule is not a Landsat 8/9 Collection 2 Level-2 SR product");
    }
    Ok(LandsatIdentity {
        acquisition,
        platform,
        processing: processing.to_owned(),
        path_row: path_row.to_owned(),
        collection: "02".into(),
        tier: parts[tier_index].to_owned(),
    })
}

fn validate_landsat_identity(
    granule: &str,
    platform: Option<&str>,
    processing: Option<&str>,
) -> Result<()> {
    let identity = parse_landsat_identity(granule)?;
    if let Some(value) = processing {
        if !eq_identity(value, &identity.processing) {
            return fail("processing level contradicts Landsat granule identifier");
        }
    }
    if let Some(value) = platform {
        if !eq_identity(value, identity.platform) {
            return fail("platform contradicts Landsat granule identifier");
        }
    }
    Ok(())
}

fn validate_parsed_platform(expected: &str, platform: Option<&str>) -> Result<()> {
    let Some(platform) = platform else {
        return Ok(());
    };
    if eq_identity(platform, expected) {
        Ok(())
    } else {
        fail("platform contradicts granule identifier")
    }
}

fn eq_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn eq_identity(a: &str, b: &str) -> bool {
    fn normalized(value: &str) -> String {
        value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    normalized(a) == normalized(b)
}
