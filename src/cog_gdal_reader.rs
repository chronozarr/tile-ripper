use super::*;
/// A worker-local COG reader. GDAL datasets and geometries never cross worker
/// threads; conditional headers are scoped around each open and read operation.
pub(crate) struct GdalCogReader {
    asset: Asset,
    dataset: gdal::Dataset,
    layout: Layout,
    band_index: usize,
    mask_geometry: Option<gdal::vector::Geometry>,
}

impl GdalCogReader {
    pub(crate) fn new(asset: &Asset, selected: &Selected) -> Result<Self> {
        let layout = selected.layout.as_ref().ok_or("missing layout")?.clone();
        let Variable::Band { index } = &layout.variable else {
            return fail("COG band missing");
        };
        let index = *index;
        let mask_geometry = selected
            .mask_geometry
            .as_ref()
            .map(|v| gdal::vector::Geometry::from_geojson(&v.to_string()))
            .transpose()?;
        let dataset = {
            let _headers = cog::ConditionalHeaders::for_asset(asset)?;
            let ds = cog::open(asset)?;
            if ds.raster_size() != (layout.width, layout.height)
                || ds.geo_transform()? != layout.transform
            {
                return fail("asset grid changed since planning");
            }
            ds.rasterband(index)?;
            ds
        };
        Ok(Self {
            asset: asset.clone(),
            dataset,
            layout,
            band_index: index,
            mask_geometry,
        })
    }

    pub(crate) fn execute_node(
        &mut self,
        node: &Node,
        report: &mut ExecutionReport,
        sink: &mut impl FnMut(Chunk) -> Result<()>,
    ) -> Result<()> {
        if node.asset_id != self.asset.id {
            return Ok(());
        }
        let Operation::Read {
            read:
                Read::LibraryWindow {
                    window,
                    band: band_index,
                    ..
                },
        } = &node.operation
        else {
            return Ok(());
        };
        if *band_index != self.band_index {
            return Ok(());
        }
        let _headers = cog::ConditionalHeaders::for_asset(&self.asset)?;
        let band = self.dataset.rasterband(self.band_index)?;
        let t = Instant::now();
        let mut pixels = band
            .read_as::<u16>(
                (window.x as isize, window.y as isize),
                (window.width, window.height),
                (window.width, window.height),
                None,
            )
            .map_err(|_| "GDAL window read failed (locator omitted)")?;
        report.read_decode_seconds += t.elapsed().as_secs_f64();
        let t = Instant::now();
        if let Some(g) = &self.mask_geometry {
            let mask = crate::spatial::mask(g, &self.layout, window)?;
            let nodata = self.layout.nodata.ok_or("AOI mask requires nodata")? as u16;
            for (value, valid) in pixels.data_mut().iter_mut().zip(mask.data()) {
                if *valid == 0 {
                    *value = nodata;
                }
            }
        }
        report.masking_seconds += t.elapsed().as_secs_f64();
        report.decoded_bytes += pixels.data().len() * 2;
        report.non_nodata_pixels += pixels
            .data()
            .iter()
            .filter(|v| self.layout.nodata.is_none_or(|n| f64::from(**v) != n))
            .count();
        let mut transform = self.layout.transform;
        transform[0] += window.x as f64 * transform[1];
        transform[3] += window.y as f64 * transform[5];
        let chunk = Chunk {
            node_id: node.id,
            asset_id: self.asset.id.clone(),
            datetime: self.asset.datetime.clone(),
            variable: self.layout.variable.clone(),
            indices: vec![],
            window: window.clone(),
            transform,
            crs: self.layout.crs.clone(),
            nodata: self.layout.nodata,
            scale: self.layout.scale,
            offset: self.layout.offset,
            pixels: pixels.into_shape_and_vec().1.into(),
            coordinates: None,
        };
        let t = Instant::now();
        sink(chunk)?;
        report.sink_seconds += t.elapsed().as_secs_f64();
        report.chunks += 1;
        Ok(())
    }
}
