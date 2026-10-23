//! Explicit candidate-to-job bridge. The existing job planner is the scientific
//! and resource-admission authority; no CMR footprint is converted to indices.
use super::*;
use crate::{
    swot, swot_job, Asset, Budget, Format, Grid, Request, Sink, Source, Spatial, Variable,
};
use serde_json::json;

pub use crate::swot_aoi::NativeWindow;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub concept_id: String,
    pub observation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<NativeWindow>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "geographic_input"
    )]
    pub aoi: Option<Spatial>,
    pub quality_policy: swot::Policy,
    pub expected_etag: Option<String>,
}
/// Accept the existing Spatial schema, or explicit CRS plus a normal GeoJSON
/// Polygon geometry. Both normalize to one stable geographic representation.
fn geographic_input<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Spatial>, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PolygonInput {
        #[serde(rename = "type")]
        kind: String,
        coordinates: Value,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GeoJsonInput {
        crs: String,
        geometry: PolygonInput,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Spatial(Spatial),
        GeoJson(GeoJsonInput),
    }
    let input = Option::<Input>::deserialize(deserializer)?;
    input
        .map(|input| match input {
            Input::Spatial(spatial) => Ok(spatial),
            Input::GeoJson(input) if input.geometry.kind == "Polygon" => Ok(Spatial::Polygon {
                crs: input.crs,
                coordinates: input.geometry.coordinates,
            }),
            _ => Err(serde::de::Error::custom(
                "AOI GeoJSON geometry must be a simple Polygon",
            )),
        })
        .transpose()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSelection {
    pub observations: Vec<Selection>,
    pub source_budget: Budget,
    pub limits: swot_job::Limits,
}
/// Uninspected requests. This is deliberately not an admitted/ready job.
pub struct Draft {
    manifest: swot_job::Manifest,
    geographic: Vec<Option<Spatial>>,
    construction_admission_seconds: f64,
}
/// An inspected selection is ready for the explicit native indices at the
/// recorded source versions. This does not claim useful WSE or river coverage.
#[derive(Debug, Serialize)]
pub struct ReadyJob {
    /// None if any explicitly selected candidate contains zero AOI centers.
    /// No replacement or silent omission is allowed.
    pub manifest: Option<swot_job::Manifest>,
    pub review: Value,
}
pub fn draft(catalogs: &[Report], selection: &JobSelection) -> Result<Draft> {
    let started = Instant::now();
    if !(1..=4).contains(&catalogs.len()) || !(1..=4).contains(&selection.observations.len()) {
        return fail(
            "explicitly choose one to four granules from one to four bounded discovery reports",
        );
    }
    let mut observations = vec![];
    for selected in &selection.observations {
        let spatial = match (&selected.window, &selected.aoi) {
            (Some(window), None) => window.spatial(),
            (None, Some(aoi)) => {
                crate::swot_aoi::polygon(aoi)?;
                // Private inspection probe, never exposed as an executable
                // geographic manifest or used to stream scientific values.
                NativeWindow {
                    start: [0, 0],
                    count: [1, 1],
                    step: [1, 1],
                }
                .spatial()
            }
            _ => {
                return fail("choose exactly one selection method per granule: window or WGS84 aoi")
            }
        };
        let mut matching = vec![];
        for report in catalogs {
            report.query.validate()?;
            if report.discovery_schema != 1
                || report.status == Completion::Failed
                || report.candidates.len() > report.query.limit
            {
                return fail("only valid completed/truncated discovery reports can supply an explicit selection");
            }
            for c in &report.candidates {
                if c.concept_id == selected.concept_id {
                    matching.push((report, c));
                }
            }
        }
        let Some((report, c)) = matching.first().copied() else {
            return fail("selected CMR granule identity is absent");
        };
        if matching
            .iter()
            .any(|(r, other)| *other != c || r.collection != report.collection)
        {
            return fail("selected CMR identity conflicts across discovery reports");
        }
        if !c.exclusions.is_empty()
            || c.data_access != AccessStatus::SupportedLocator
            || c.processing_family != FamilyStatus::AppearsRasterDPgd0
        {
            return fail("selected candidate is excluded or has ambiguous/unsupported catalog identity or access");
        }
        let p = c
            .product
            .as_ref()
            .ok_or("selected product identity unresolved")?;
        if ProductIdentity::parse(c.granule_ur.as_deref().ok_or("missing granule UR")?)? != *p {
            return fail("selected product identity conflicts with granule UR");
        }
        let catalog = CatalogProvenance {
            collection: report
                .collection
                .clone()
                .ok_or("collection identity unresolved")?,
            concept_id: c.concept_id.clone(),
            revision: c.revision,
            granule_ur: c.granule_ur.clone().unwrap(),
            acquisition_start: c.acquisition_start.clone().ok_or("missing catalog start")?,
            acquisition_end: c.acquisition_end.clone().ok_or("missing catalog end")?,
            processing_release: p.processing_release.clone(),
            product_counter: p.product_counter.clone(),
            pge_version: c.pge_version.clone(),
            source_size_bytes: c.source_size_bytes,
        };
        let asset = Asset {
            id: c.concept_id.clone(),
            locator: c.source_url.clone().ok_or("missing source locator")?,
            format: Format::Hdf5,
            datetime: Some(catalog.acquisition_start.clone()),
            etag: selected.expected_etag.clone(),
        };
        catalog.validate(&asset)?;
        observations.push(swot_job::Observation {
            id: selected.observation_id.clone(),
            quality_policy: selected.quality_policy.clone(),
            catalog: Some(catalog),
            geographic_selection: None,
            request: Request {
                source: Source::Assets {
                    assets: vec![asset],
                },
                spatial,
                time: None,
                variables: swot::PATHS
                    .iter()
                    .map(|p| Variable::Dataset { path: (*p).into() })
                    .collect(),
                quality: vec![],
                grid: Grid::Native,
                operations: vec![],
                sink: Sink::Callback,
                budget: selection.source_budget.clone(),
            },
        });
    }
    let manifest = swot_job::Manifest {
        job_schema: 1,
        profile: swot_job::Profile::SwotRasterDPgd0,
        observations,
        limits: selection.limits.clone(),
    };
    manifest.validate()?;
    Ok(Draft {
        manifest,
        geographic: selection
            .observations
            .iter()
            .map(|s| s.aoi.clone())
            .collect(),
        construction_admission_seconds: started.elapsed().as_secs_f64(),
    })
}
impl Draft {
    /// Reviewable draft only: no source compatibility or native-index admission
    /// has been established. Execute normal Job::plan before any readiness claim.
    pub fn uninspected_manifest(&self) -> Result<&swot_job::Manifest> {
        if self.geographic.iter().any(Option::is_some) {
            return fail("geographic draft has no executable manifest until actual source axes are inspected");
        }
        Ok(&self.manifest)
    }
    pub fn construction_admission_seconds(&self) -> f64 {
        self.construction_admission_seconds
    }
    /// Native metadata inspection through a caller-owned fresh exact-object
    /// authentication factory; no default credentials and no scientific stream.
    #[cfg(feature = "authenticated-http")]
    pub fn inspect_authenticated(
        mut self,
        sessions: &mut impl FnMut(
            &Asset,
        ) -> Result<
            std::sync::Arc<crate::authenticated_http::AuthenticatedHttp>,
        >,
    ) -> Result<ReadyJob> {
        let started = Instant::now();
        let mut resolutions = Vec::new();
        let mut empty = false;
        for (entry, aoi) in self.manifest.observations.iter_mut().zip(&self.geographic) {
            if let Some(aoi) = aoi {
                let mut cache = crate::cog::LayoutCache::default();
                let resolution = crate::swot_aoi::inspect_with(entry, aoi, |request| {
                    let Source::Assets { assets } = &request.source else {
                        unreachable!()
                    };
                    let session = sessions(&assets[0])?;
                    session.require_fresh()?;
                    crate::acquisition::prepare_authenticated(request, &mut cache, session)
                })?;
                empty |= !resolution["native_centers_intersect"]
                    .as_bool()
                    .unwrap_or(false);
                resolutions.push(json!({"observation_id":entry.id,"concept_id":entry.asset()?.id,"aoi_resolution":resolution}));
            }
        }
        if empty {
            return Ok(ReadyJob {
                manifest: None,
                review: json!({"discovery_job_schema":1,"status":"not_ready_zero_native_centers",
                "observations":resolutions,"inspection_seconds":started.elapsed().as_secs_f64(),
                "manifest_written":false,"scope":"at least one explicitly selected candidate has zero included native centers; no candidate substituted or omitted; final selected five-variable scientific profile not inspected"}),
            });
        }
        let plan = swot_job::Job::authenticated(&self.manifest, sessions, None)?.plan()?;
        let inspection_seconds = started.elapsed().as_secs_f64();
        let mut sources = vec![];
        for (entry, inspected) in self.manifest.observations.iter_mut().zip(
            plan["observations"]
                .as_array()
                .ok_or("missing job inspections")?,
        ) {
            let source = &inspected["inspection"]["sources"][0]["object_identities"];
            let etag = source["identity"]["version"]["etag"]
                .as_str()
                .ok_or("source inspection requires a strong ETag")?
                .to_owned();
            let Source::Assets { assets } = &mut entry.request.source else {
                unreachable!()
            };
            assets[0].etag = Some(etag);
            let datasets = source["datasets"]
                .as_array()
                .ok_or("missing source datasets")?;
            let d = &datasets[0];
            let geographic = resolutions
                .iter()
                .find(|r| r["observation_id"] == entry.id)
                .map(|r| r["aoi_resolution"].clone());
            sources.push(json!({"observation_id":entry.id,"concept_id":assets[0].id,"aoi_resolution":geographic,
                "scientific_profile":"passed_existing_native_profile","selection":"admitted_explicit_native_indices",
                "useful_selected_wse":"unknown; metadata inspection does not read science pixels or establish river coverage",
                "catalog":entry.catalog,"source_identity":source["identity"],"source_shape":d["shape"],
                "source_root_attributes":d["root_attributes"],"native_crs":d["grid_mapping"],"coordinate_definitions":d["axes"],
                "native_coordinate_ranges":{"x_min":d["root_attributes"]["x_min"],"x_max":d["root_attributes"]["x_max"],"y_min":d["root_attributes"]["y_min"],"y_max":d["root_attributes"]["y_max"]},
                "explicit_selection":entry.request.spatial,"quality_policy":entry.quality_policy,
                "geographic_selection":entry.geographic_selection,
                "catalog_and_native_time":"separately retained; catalog granule times and native coverage times may differ",
                "inspection_transport":inspected["inspection"]["transport"]}));
        }
        self.manifest.validate()?;
        Ok(ReadyJob {
            manifest: Some(self.manifest),
            review: json!({"discovery_job_schema":1,"construction_admission_seconds":self.construction_admission_seconds,
            "status":"ready_for_explicit_native_index_job","inspection_seconds":inspection_seconds,
            "observations":sources,"admission":plan["admission"],"native_plan":plan,
            "scope":"metadata/profile/native-center selection admission at inspected source versions; fetch reauthenticates and revalidates. No science-pixel inspection or useful river-WSE assertion."}),
        })
    }
}
/// Called by normal job plan AND fetch whenever catalog assertions are present.
/// Scientific metadata is read by the existing native preparation path.
pub(crate) fn validate_source(c: &CatalogProvenance, a: &Asset, inspection: &Value) -> Result<()> {
    let p = c.validate(a)?;
    let source = &inspection["sources"][0]["object_identities"];
    let ds = source["datasets"]
        .as_array()
        .ok_or("missing inspected source identity")?;
    let d = ds
        .iter()
        .find(|d| d["path"] == "/wse")
        .ok_or("missing native WSE identity")?;
    let root = &d["root_attributes"];
    if root["crid"]["value"] != p.processing_release
        || root["product_version"]["value"] != p.product_counter
        || root["descriptor_string"]["value"] != p.descriptor
        || root["cycle_number"]["value"].as_f64() != Some(p.cycle as f64)
        || root["pass_number"]["value"].as_f64() != Some(p.pass as f64)
        || root["scene_number"]["value"].as_f64() != Some(p.scene as f64)
        || c.pge_version
            .as_ref()
            .is_some_and(|v| root["pge_version"]["value"] != *v)
        || c.source_size_bytes
            .is_some_and(|n| source["identity"]["version"]["size"].as_u64() != Some(n))
    {
        return fail("native source identity conflicts with selected catalog product, processing version or size");
    }
    // Product filenames round the granule endpoints to whole seconds. Keep
    // catalog times and native coverage/granule times separately in provenance.
    for (key, expected) in [
        ("time_granule_start", &p.filename_start),
        ("time_granule_end", &p.filename_end),
    ] {
        let actual = datetime(
            root[key]["value"]
                .as_str()
                .ok_or("native source lacks granule times")?,
        )?;
        if (actual - datetime(expected)?).num_milliseconds().abs() > 1000 {
            return fail("native source granule time conflicts with canonical product identity");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_identity_assertions_refuse_conflicts_without_inventing_science() {
        let name="SWOT_L2_HR_Raster_100m_UTM19T_N_x_x_x_011_035_115F_20240216T025153_20240216T025214_PGD0_01";
        let c = CatalogProvenance {
            collection: CollectionIdentity {
                concept_id: "C3233944993-POCLOUD".into(),
                revision: 24,
                short_name: COLLECTION.into(),
                version: "D".into(),
                provider: "POCLOUD".into(),
            },
            concept_id: "G3927359490-POCLOUD".into(),
            revision: 2,
            granule_ur: name.into(),
            acquisition_start: "2024-02-16T02:51:53.691Z".into(),
            acquisition_end: "2024-02-16T02:52:14.795Z".into(),
            processing_release: "PGD0".into(),
            product_counter: "01".into(),
            pge_version: Some("5.4.0".into()),
            source_size_bytes: Some(89242093),
        };
        let a = Asset {
            id: c.concept_id.clone(),
            locator: format!("{ARCHIVE}{name}.nc"),
            format: Format::Hdf5,
            datetime: Some(c.acquisition_start.clone()),
            etag: None,
        };
        let root = json!({"crid":{"value":"PGD0"},"product_version":{"value":"01"},"descriptor_string":{"value":"100m_UTM19T_N_x_x_x"},"cycle_number":{"value":11},"pass_number":{"value":35},"scene_number":{"value":115},"pge_version":{"value":"5.4.0"},"time_granule_start":{"value":"2024-02-16T02:51:53.691Z"},"time_granule_end":{"value":"2024-02-16T02:52:14.795Z"}});
        let good = json!({"sources":[{"object_identities":{"identity":{"version":{"size":89242093}},"datasets":[{"path":"/wse","root_attributes":root}]}}]});
        validate_source(&c, &a, &good).unwrap();
        for (key, value) in [
            ("crid", json!("PID0")),
            ("product_version", json!("02")),
            ("descriptor_string", json!("100m_UTM01W_N_x_x_x")),
            ("cycle_number", json!(12)),
            ("pass_number", json!(36)),
            ("scene_number", json!(116)),
            ("pge_version", json!("9.0")),
            ("time_granule_start", json!("2024-02-17T00:00:00Z")),
            ("time_granule_end", json!("2024-02-17T00:00:00Z")),
        ] {
            let mut bad = good.clone();
            bad["sources"][0]["object_identities"]["datasets"][0]["root_attributes"][key]
                ["value"] = value;
            assert!(validate_source(&c, &a, &bad).is_err(), "{key}");
        }
        let mut bad = good.clone();
        bad["sources"][0]["object_identities"]["identity"]["version"]["size"] = json!(10);
        assert!(validate_source(&c, &a, &bad).is_err());
        let mut bad_catalog = c.clone();
        bad_catalog.acquisition_start = "2020-01-01T00:00:00Z".into();
        assert!(bad_catalog.validate(&a).is_err());
    }
}
