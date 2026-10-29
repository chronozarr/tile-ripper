//! Closed, conservative source policy. A scientific profile is independent of
//! its physical distribution; changing a provider always requires a new plan.
use super::{profile, Observation, ProductRequest};
use crate::{fail, Result};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const POLICY: &str = "qualified-provider-v1";
pub const SIGNING_POLICY: &str = "pc-exact-object-request-v1";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Auto,
    PlanetaryComputer,
    Earthdata,
    /// Explicit supplied local or independent public mirrors, never discovered.
    Pinned,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::PlanetaryComputer => "planetary_computer",
            Self::Earthdata => "earthdata",
            Self::Pinned => "pinned",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub provider: Provider,
    pub catalog: &'static str,
    pub product: String,
    pub processing_version: &'static str,
    pub discovery: &'static str,
    pub access: &'static str,
    pub credential_requirement: &'static str,
    pub access_policy: &'static str,
    pub capabilities: &'static str,
    pub availability: &'static str,
    pub limitations: &'static str,
}
#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub policy: &'static str,
    pub requested: Provider,
    pub selected: Provider,
    pub basis: &'static str,
    pub rationale: &'static str,
    pub credentials_required: bool,
    pub authorization_configured: bool,
    pub observations_preserved: bool,
    pub routes: Vec<Route>,
    pub observation_count: usize,
    pub eligibility: serde_json::Value,
    pub equivalence: serde_json::Value,
    pub pinning: &'static str,
    pub fallback: &'static str,
}
fn supports_pc(product: &str) -> bool {
    matches!(
        product,
        "landsat-c2-l2-sr" | "landsat-tm-c2-l2-sr" | "landsat-etm-c2-l2-sr" | "hls-l30" | "hls-s30"
    )
}
pub fn routes(product: &str) -> Result<Vec<Route>> {
    let p = profile::profile(product)?;
    let mut out = Vec::new();
    if p.family == profile::ProductFamily::OperaDswxNiPrototype {
        return Ok(vec![Route {
            provider: Provider::Pinned,
            catalog: "explicit pinned public engineering objects; no production catalog",
            product: p.id.into(),
            processing_version: p.version,
            discovery: "unsupported; supply the separately qualified prototype observation",
            access: "anonymous conditional HTTPS ranges confined to pinned prototype objects",
            credential_requirement: "none",
            access_policy: "dswx-ni-prototype-pinned-v0.1",
            capabilities: "native UInt8 COG values, separate source/AOI/optional class masks",
            availability: "one version-pinned public engineering specimen; not final NI v1.0",
            limitations: "no production discovery, archive authentication, provider fallback or S1-policy alias",
        }]);
    }
    if supports_pc(p.id) {
        out.push(Route { provider: Provider::PlanetaryComputer,
            catalog: "https://planetarycomputer.microsoft.com/api/stac/v1",
            product:p.id.into(), processing_version:p.version,
            discovery:"bounded complete STAC search; hls2-l30/hls2-s30 only for HLS v2",
            access:"accountless memory-only SAS signing; observed container read/list grant; engine requests remain confined to the exact canonical object with conditional byte ranges",
            credential_requirement:"none", access_policy:SIGNING_POLICY,
            capabilities:"native COG; physical metadata and ETag validation before/after acquisition",
            availability:"queried AOI/date only; catalog coverage is independent of Earthdata; no global coverage claim",
            limitations:"requires signing service; HLS cross-provider equivalence is specimen-scoped; no online speed probe or fallback" });
    }
    if p.family != profile::ProductFamily::Landsat {
        out.push(Route { provider:Provider::Earthdata, catalog:"https://cmr.earthdata.nasa.gov/search",
            product:p.id.into(), processing_version:p.version, discovery:"exact CMR collection/version and complete bounded granule search",
            access:"product-specific exact-object Earthdata session; qualification scope is recorded per product",
            credential_requirement:"explicit private netrc", access_policy:"earthdata-product-exact-object-v1",
            capabilities:"native COG; conditional byte ranges; product-specific redirect confinement",
            availability:"queried AOI/date only; missing observations are not substituted",
            limitations:"authentication required for protected assets; no demonstrated general speed advantage" });
    }
    Ok(out)
}
fn locator_provider(href: &str) -> Result<Provider> {
    if let Ok(url) = Url::parse(href) {
        if !matches!(url.scheme(), "http" | "https" | "file") {
            return fail("unsupported product source scheme");
        }
        if url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return fail("product sources require canonical unsigned URLs without query, fragment or credentials");
        }
        let host = url.host_str().unwrap_or_default();
        if host.ends_with(".blob.core.windows.net") {
            return Ok(Provider::PlanetaryComputer);
        }
        if ["data.asf.alaska.edu", "datapool.asf.alaska.edu"].contains(&host) {
            return fail("legacy ASF origin has no qualified Earthdata authentication route; explicitly select a current CMR Cumulus record or a qualified local mirror");
        }
        // Keep these exact origins aligned with the owning product-specific
        // authenticated_http constructors; redirects are confined there.
        if [
            "data.lpdaac.earthdatacloud.nasa.gov",
            "archive.podaac.earthdata.nasa.gov",
            "cumulus.asf.earthdatacloud.nasa.gov",
        ]
        .contains(&host)
        {
            if url.scheme() != "https" || url.port().is_some() {
                return fail(
                    "qualified Earthdata product routing requires the exact HTTPS archive origin",
                );
            }
            return Ok(Provider::Earthdata);
        }
    }
    Ok(Provider::Pinned)
}
pub(crate) fn select(r: &ProductRequest, _authorized: bool) -> Result<Provider> {
    let product = profile::profile(&r.product)?.id;
    if product == super::dswx_ni::ID && (r.provider != Provider::Pinned || r.observations.is_none())
    {
        return fail("DSWx-NI prototype requires provider pinned and explicit qualified observations; no production discovery is supported");
    }
    if let Some(observations) = &r.observations {
        let mut providers = BTreeSet::new();
        for o in observations {
            for requested in &r.bands {
                let band = profile::profile(product)?.band(requested)?.name.clone();
                let asset = o
                    .assets
                    .get(&band)
                    .ok_or("selected observation missing requested canonical band asset")?;
                providers.insert(locator_provider(&asset.href)?);
            }
        }
        if providers.len() > 1 {
            return fail("ambiguous supplied observations mix physical providers; split into separately prepared acquisitions");
        }
        let actual = providers.first().copied().unwrap_or(Provider::Pinned);
        if r.provider != Provider::Auto && r.provider != actual && !observations.is_empty() {
            return fail("explicit provider conflicts with supplied physical sources; no substitution is allowed; explicitly create a fresh discovery request");
        }
        return Ok(if observations.is_empty() && r.provider != Provider::Auto {
            r.provider
        } else {
            actual
        });
    }
    let selected = match r.provider {
        Provider::Auto if supports_pc(product) => Provider::PlanetaryComputer,
        Provider::Auto => Provider::Earthdata,
        Provider::Pinned => return fail("pinned provider requires explicit observations"),
        provider => provider,
    };
    if !routes(product)?
        .iter()
        .any(|route| route.provider == selected)
    {
        return fail("explicit provider has no qualified route for requested product; providers are never switched silently");
    }
    Ok(selected)
}
pub(crate) fn validate_source(
    r: &ProductRequest,
    selected: Provider,
    o: &Observation,
) -> Result<()> {
    for requested in &r.bands {
        let band = profile::profile(&r.product)?.band(requested)?.name.clone();
        let a = &o.assets[&band];
        if locator_provider(&a.href)? != selected {
            return fail("catalog asset provider conflicts with selected route");
        }
        if selected == Provider::PlanetaryComputer {
            validate_planetary_locator(&r.product, &a.href)?;
        }
        if selected == Provider::Earthdata
            && matches!(r.product.as_str(), "opera-dswx-s1" | "opera-dist-alert")
        {
            let dist = r.product == "opera-dist-alert";
            let u = Url::parse(&a.href)?;
            let layer = if dist {
                band.clone()
            } else {
                format!(
                    "B{:02}_{band}",
                    match band.as_str() {
                        "WTR" => 1,
                        "BWTR" => 2,
                        "CONF" => 3,
                        "DIAG" => 4,
                        _ => return fail("invalid DSWx-S1 band"),
                    }
                )
            };
            let expected = if dist {
                format!(
                    "/lp-prod-protected/OPERA_L3_DIST-ALERT-HLS_V1/{0}/{0}_{layer}.tif",
                    o.id
                )
            } else {
                format!(
                    "/podaac-ops-cumulus-protected/OPERA_L3_DSWX-S1_V1/{}_{layer}.tif",
                    o.id
                )
            };
            let host = if dist {
                "data.lpdaac.earthdatacloud.nasa.gov"
            } else {
                "archive.podaac.earthdata.nasa.gov"
            };
            if u.host_str() != Some(host) || u.path() != expected {
                return fail("Wave 2 catalog locator conflicts with exact product/granule/layer");
            }
        }
        if matches!(selected, Provider::PlanetaryComputer | Provider::Earthdata)
            && r.product.starts_with("hls-")
        {
            let path = Url::parse(&a.href)?.path().to_owned();
            if !path.ends_with(&format!("/{0}/{0}.{1}.tif", o.id, band)) {
                return fail("HLS physical locator conflicts with granule/band identity");
            }
        }
    }
    Ok(())
}
pub fn validate_planetary_locator(product: &str, href: &str) -> Result<()> {
    let u = Url::parse(href)?;
    let allowed = match profile::profile(product)?.id {
        "landsat-c2-l2-sr" | "landsat-tm-c2-l2-sr" | "landsat-etm-c2-l2-sr" => {
            u.host_str()
                .is_some_and(|h| ["landsateuwest.blob.core.windows.net"].contains(&h))
                && u.path().starts_with("/landsat-c2/")
        }
        "hls-l30" => {
            u.host_str() == Some("hls2euwest.blob.core.windows.net")
                && u.path().starts_with("/hls2/L30/")
        }
        "hls-s30" => {
            u.host_str() == Some("hls2euwest.blob.core.windows.net")
                && u.path().starts_with("/hls2/S30/")
        }
        _ => false,
    };
    if !allowed
        || u.scheme() != "https"
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return fail(
            "Planetary Computer canonical object violates qualified product signing policy",
        );
    }
    Ok(())
}
/// Validate an untrusted signer response without exposing its bearer query.
pub fn validate_signature(canonical: &str, signed: &str) -> Result<()> {
    let u = Url::parse(canonical)?;
    let s = Url::parse(signed).map_err(|_| "signer returned invalid URL")?;
    if u.scheme() != s.scheme()
        || u.host_str() != s.host_str()
        || u.port() != s.port()
        || u.path() != s.path()
        || !s.username().is_empty()
        || s.password().is_some()
        || s.fragment().is_some()
    {
        return fail(
            "signing changed canonical physical object; explicitly replan, no provider fallback",
        );
    }
    let mut q = std::collections::BTreeMap::new();
    for (k, v) in s.query_pairs() {
        // SAS authorization may not add Azure operations, snapshots or version
        // selectors to a canonical object request (for example comp=list).
        if ![
            "sp", "st", "se", "sv", "sr", "sig", "skoid", "sktid", "skt", "ske", "sks", "skv",
            "sip", "spr", "si",
        ]
        .contains(&k.as_ref())
        {
            return fail("signer added an unqualified query operation or object selector");
        }
        if q.insert(k.into_owned(), v.into_owned()).is_some() {
            return fail("signer returned ambiguous duplicate SAS fields");
        }
    }
    // Current public signing returns a container-scoped read/list SAS. The
    // engine never uses its listing capability: every request retains the exact
    // canonical object URL and immutable ETag. Also admit exact read-only blobs.
    let grant = (
        q.get("sp").map(String::as_str),
        q.get("sr").map(String::as_str),
    );
    if !matches!(grant, (Some("r"), Some("b")) | (Some("rl"), Some("c")))
        || q.get("sig").is_none_or(String::is_empty)
    {
        return fail("signer returned an unqualified SAS grant; expected read-only blob or observed read/list container grant, with engine requests confined to the exact canonical object");
    }
    let expires = q
        .get("se")
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .ok_or("signer missing valid expiration")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    if q.get("st").is_some_and(|start| {
        chrono::DateTime::parse_from_rfc3339(start)
            .map_or(true, |start| start.timestamp() > now as i64 + 30)
    }) {
        return fail("Planetary Computer signature is not yet valid; explicitly replan when the signing policy permits access");
    }
    if expires.timestamp() <= now as i64 + 30 {
        return fail("Planetary Computer signature expired or too near expiry; explicitly create a fresh plan for the same canonical object");
    }
    Ok(())
}

/// Bearer grants live only for one product preparation. A verified container
/// grant may authorize another exact object in that same host/container; a
/// blob grant is reusable only for its original canonical object. No token is
/// serialized, refreshed, or used to list the container.
#[derive(Default)]
pub(crate) struct PlanetarySignatures {
    containers: BTreeMap<(String, String), String>,
    blobs: BTreeMap<String, String>,
}
impl PlanetarySignatures {
    pub(crate) fn sign(
        &mut self,
        product: &str,
        canonical: &str,
        fetch: impl FnOnce() -> Result<String>,
    ) -> Result<String> {
        validate_planetary_locator(product, canonical)?;
        let mut url = Url::parse(canonical)?;
        let scope = (
            url.host_str()
                .ok_or("canonical object has no host")?
                .to_owned(),
            url.path_segments()
                .and_then(|mut p| p.next())
                .filter(|s| !s.is_empty())
                .ok_or("canonical object has no container")?
                .to_owned(),
        );
        if let Some(query) = self
            .blobs
            .get(canonical)
            .or_else(|| self.containers.get(&scope))
        {
            url.set_query(Some(query));
            let signed = url.to_string();
            // Revalidate object identity, grant and expiry on every use. An
            // expiring grant fails the preparation rather than fetching a new
            // authorization halfway through its already selected sources.
            validate_signature(canonical, &signed)?;
            return Ok(signed);
        }
        let signed = fetch()?;
        validate_signature(canonical, &signed)?;
        let parsed = Url::parse(&signed).map_err(|_| "signer returned invalid URL")?;
        let query = parsed
            .query()
            .ok_or("signer returned no SAS query")?
            .to_owned();
        if parsed
            .query_pairs()
            .any(|(key, value)| key == "sr" && value == "c")
        {
            self.containers.insert(scope, query);
        } else {
            self.blobs.insert(canonical.to_owned(), query);
        }
        Ok(signed)
    }
}
pub(crate) fn decision(
    r: &ProductRequest,
    selected: Provider,
    authorized: bool,
    observations: &[Observation],
) -> Result<Decision> {
    let mut available = routes(&r.product)?;
    if selected == Provider::Pinned {
        available.clear();
    }
    Ok(Decision { policy:POLICY, requested:r.provider, selected,
        basis:if r.provider != Provider::Auto {"explicit_override"} else if r.observations.is_some() {"supplied_physical_sources"} else if r.product.starts_with("hls-") {"bounded_catalog_coverage_then_conservative_default"} else {"conservative_static_default"},
        rationale:if r.observations.is_some() {"Supplied observations and canonical physical locators are preserved; no remote substitution."} else if r.provider == Provider::Auto && r.product.starts_with("hls-") {"Complete bounded catalog coverage is compared before the credential-free preference. See eligibility for exact observed IDs, failures and the selected coverage decision."} else if selected == Provider::PlanetaryComputer {"Qualified credential-free discovery is preferred; no conclusive material authenticated-route advantage is established."} else {"This product has no qualified credential-free discovered alternative; explicit Earthdata authorization is required for protected assets."},
        credentials_required:selected == Provider::Earthdata, authorization_configured:authorized,
        eligibility:serde_json::json!({"selected_source_observations":observations.len(),"selected_discovery_complete":true,"alternatives":"not queried; route existence is not evidence of matched observation coverage","access_eligible_providers":available.iter().filter(|route|route.provider != Provider::Earthdata || authorized).map(|route|route.provider).collect::<Vec<_>>()}),
        observations_preserved:r.observations.is_some(), routes:available, observation_count:observations.len(),
        equivalence:serde_json::json!({"classification":"unknown_across_providers","group":null,"scope":"Physical product identity is validated for every source. Matching product names or timestamps alone never establish cross-provider equivalence; no observation is replaced."}),
        pinning:"provider, unsigned canonical locator, product/granule/band, access-policy version and inspected object ETag/grid/dtype/metadata are bound to this prepared acquisition; see sources for exact identities",
        fallback:"disabled; signature renewal/provider change requires an explicit fresh plan; product checkpoints are unsupported" })
}

/// Evidence is a compiled, closed specimen set. User metadata and product names
/// cannot create a group; both canonical locator and physically observed strong
/// ETag must match an independently compared object.
pub fn attestation(
    product: &str,
    granule: &str,
    band: &str,
    canonical: &str,
    etag: Option<&str>,
) -> serde_json::Value {
    let evidence: serde_json::Value = serde_json::from_str(include_str!("equivalence.json"))
        .expect("compiled qualification evidence");
    for case in evidence["cases"].as_array().into_iter().flatten() {
        if case["product"] != product || case["granule"] != granule {
            continue;
        }
        for source in case["sources"].as_array().into_iter().flatten() {
            if source["band"] == band
                && source["href"] == canonical
                && etag.is_some()
                && source["etag"].as_str() == etag
            {
                return serde_json::json!({"classification":"scientifically_equivalent_selected_sample","group":format!("{granule}/{band}"),"window":case["window"],"scope":case["scope"],"binding":"exact canonical locator and inspected strong physical ETag","evidence":"docs/providers/equivalence.json","exact_byte_replica_proven":false});
            }
        }
    }
    serde_json::json!({"classification":"unknown_across_providers","group":null,"scope":"No independent immutable-object sample attestation for this source; no substitution permitted."})
}

/// Pure coverage policy over independently complete, product-validated catalogs.
/// None means failed or incomplete discovery and never means zero observations.
pub fn choose_coverage(
    pc: Option<&BTreeSet<String>>,
    nasa: Option<&BTreeSet<String>>,
    authorized: bool,
) -> Result<(Provider, &'static str)> {
    if !authorized {
        if pc.is_some_and(|ids| !ids.is_empty()) {
            return Ok((Provider::PlanetaryComputer,"credential-free complete catalog selected; any Earthdata-only identities are disclosed and require explicit authorization"));
        }
        if nasa.is_some_and(|ids| !ids.is_empty()) {
            return Ok((Provider::Earthdata,"no credential-free observations; Earthdata coverage discovered but explicit authorization is required before preparation"));
        }
    }
    match (pc,nasa) {
        (Some(p),Some(n)) if p == n => Ok((Provider::PlanetaryComputer,"equal exact observation sets; conservative credential-free preference, no material measured advantage established")),
        (Some(p),Some(n)) if p.is_superset(n) => Ok((Provider::PlanetaryComputer,"Planetary Computer complete catalog is a strict observation superset")),
        (Some(p),Some(n)) if n.is_superset(p) => Ok((Provider::Earthdata,"Earthdata complete catalog is a strict observation superset; coverage takes precedence over credential-free preference")),
        (Some(_),Some(_)) => fail("qualified provider catalogs contain incomparable observation sets; specify --provider explicitly or freeze exact observations; no implicit loss or substitution"),
        (Some(_),None) => Ok((Provider::PlanetaryComputer,"only Planetary Computer returned a complete catalog; Earthdata discovery failure is disclosed, coverage comparison remains unknown")),
        (None,Some(_)) => Ok((Provider::Earthdata,"only Earthdata returned a complete catalog; Planetary Computer discovery failure is disclosed; authorization required for preparation")),
        (None,None) => fail("both qualified provider catalogs are unavailable or incomplete; narrow AOI/date/max_observations or select a provider explicitly; no acquisition was prepared"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn request(product: &str) -> ProductRequest {
        serde_json::from_value(json!({"product":product,"aoi":{"kind":"bounds","crs":"EPSG:4326","bbox":[-79.,35.,-78.,36.]},"start":"2024-06-01T00:00:00Z","end":"2024-06-30T23:59:59Z","bands":["B04"]})).unwrap()
    }
    #[test]
    fn conservative_policy_is_free_even_when_auth_is_configured() {
        for p in ["hls-l30", "hls-s30", "landsat-c2-l2-sr"] {
            assert_eq!(
                select(&request(p), false).unwrap(),
                Provider::PlanetaryComputer
            );
            assert_eq!(
                select(&request(p), true).unwrap(),
                Provider::PlanetaryComputer
            );
        }
        assert_eq!(
            select(&request("opera-dswx-hls"), false).unwrap(),
            Provider::Earthdata
        );
        let mut r = request("hls-l30");
        r.provider = Provider::Earthdata;
        assert_eq!(select(&r, false).unwrap(), Provider::Earthdata);
        r.product = "landsat-c2-l2-sr".into();
        assert!(select(&r, true).is_err());
    }
    #[test]
    fn older_hls_and_other_container_cannot_enter_signer() {
        for href in [
            "https://hls2euwest.blob.core.windows.net/hls/L30/a.tif",
            "https://hls2euwest.blob.core.windows.net/hls2/S30/a.tif",
            "https://hls2euwest.blob.core.windows.net/hls2/L30/a.tif?sig=secret",
            "https://hls2euwest.blob.core.windows.net:444/hls2/L30/a.tif",
            "https://hls2euwest.blob.core.windows.net.evil.org/hls2/L30/a.tif",
        ] {
            assert!(
                validate_planetary_locator("hls-l30", href).is_err(),
                "{href}"
            );
        }
        assert!(validate_planetary_locator(
            "hls-l30",
            "https://hls2euwest.blob.core.windows.net/hls2/L30/a.tif"
        )
        .is_ok());
    }
    #[test]
    fn signing_grants_preserve_exact_live_object_requests() {
        let c = "https://hls2euwest.blob.core.windows.net/hls2/L30/a.tif";
        let valid = format!("{c}?sp=r&sr=b&sig=secret&se=2099-01-01T00%3A00%3A00Z");
        assert!(validate_signature(c, &valid).is_ok());
        let container_grant = valid.replace("sp=r&sr=b", "sp=rl&sr=c");
        assert!(validate_signature(c, &container_grant).is_ok());
        for value in [
            container_grant.replace("sp=rl", "sp=rlw"),
            container_grant.replace("sp=rl", "sp=rlacwd"),
            container_grant.replace("sr=c", "sr=b"),
            container_grant.replace("a.tif", "b.tif"),
            container_grant.replace("2099", "2000"),
            format!("{container_grant}&comp=list&restype=container"),
            format!("{container_grant}&snapshot=2020-01-01"),
            valid.replace("a.tif", "b.tif"),
            valid.replace("sp=r", "sp=rw"),
            valid.replace("sr=b", "sr=c"),
            valid.replace("2099", "2000"),
            format!("{valid}&sp=r"),
            format!("{valid}#secret"),
            format!("{valid}&st=2099-01-01T00%3A00%3A00Z"),
            valid.replace(".net/", ".net:444/"),
        ] {
            assert!(validate_signature(c, &value).is_err());
        }
    }
    #[test]
    fn preparation_reuses_only_verified_container_grants() -> Result<()> {
        let a = "https://hls2euwest.blob.core.windows.net/hls2/S30/a.tif";
        let b = "https://hls2euwest.blob.core.windows.net/hls2/S30/b.tif";
        let query = "sp=rl&sr=c&sig=test-signature&se=2099-01-01T00%3A00%3A00Z";
        let mut cache = PlanetarySignatures::default();
        cache.sign("hls-s30", a, || Ok(format!("{a}?{query}")))?;
        let signed = cache.sign("hls-s30", b, || {
            Err("cached container must not contact signer".into())
        })?;
        assert_eq!(signed, format!("{b}?{query}"));
        validate_signature(b, &signed)?;
        assert!(cache
            .sign("hls-l30", b, || panic!(
                "invalid product must not contact signer"
            ))
            .is_err());
        let landsat = "https://landsateuwest.blob.core.windows.net/landsat-c2/a.tif";
        let mut called = false;
        cache.sign("landsat-c2-l2-sr", landsat, || {
            called = true;
            Ok(format!("{landsat}?{query}"))
        })?;
        assert!(
            called,
            "a different qualified host/container needs its own grant"
        );
        Ok(())
    }
    #[test]
    fn blob_grants_and_expired_container_grants_are_not_promoted_or_refreshed() -> Result<()> {
        let a = "https://hls2euwest.blob.core.windows.net/hls2/S30/a.tif";
        let b = "https://hls2euwest.blob.core.windows.net/hls2/S30/b.tif";
        let query = "sp=r&sr=b&sig=test-signature&se=2099-01-01T00%3A00%3A00Z";
        let mut cache = PlanetarySignatures::default();
        cache.sign("hls-s30", a, || Ok(format!("{a}?{query}")))?;
        cache.sign("hls-s30", a, || panic!("exact blob grant should be reused"))?;
        let mut called = false;
        cache.sign("hls-s30", b, || {
            called = true;
            Ok(format!("{b}?{query}"))
        })?;
        assert!(called, "a blob grant cannot authorize a different object");
        let mut cache = PlanetarySignatures::default();
        cache.sign("hls-s30", a, || {
            Ok(format!("{a}?{}", query.replace("sp=r&sr=b", "sp=rl&sr=c")))
        })?;
        *cache.containers.values_mut().next().unwrap() = query
            .replace("sp=r&sr=b", "sp=rl&sr=c")
            .replace("2099", "2000");
        assert!(cache
            .sign("hls-s30", b, || panic!(
                "expiring preparation must fail without refreshing authorization"
            ))
            .is_err());
        Ok(())
    }
    #[test]
    fn rejected_signatures_never_enter_preparation_cache() {
        let a = "https://hls2euwest.blob.core.windows.net/hls2/S30/a.tif";
        let mut cache = PlanetarySignatures::default();
        assert!(cache
            .sign("hls-s30", a, || Ok(format!(
                "{a}?sp=rlw&sr=c&sig=test&se=2099-01-01T00%3A00%3A00Z"
            )))
            .is_err());
        assert!(cache.containers.is_empty() && cache.blobs.is_empty());
        assert!(cache
            .sign("hls-s30", a, || Ok(format!(
                "{}?sp=rl&sr=c&sig=test&se=2099-01-01T00%3A00%3A00Z",
                a.replace("a.tif", "b.tif")
            )))
            .is_err());
        assert!(cache.containers.is_empty() && cache.blobs.is_empty());
    }
    #[test]
    fn product_name_alone_never_creates_equivalence() {
        let r = request("hls-l30");
        let d = decision(&r, Provider::PlanetaryComputer, false, &[]).unwrap();
        assert!(d.equivalence["group"].is_null());
        assert_eq!(d.equivalence["classification"], "unknown_across_providers");
    }
}
