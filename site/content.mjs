export const repository = 'https://github.com/chronozarr/tile-ripper';
export const origin = 'https://tileripper.com';
export const sections = [
  { text: 'Start here', pages: [
    ['docs/quickstart.md', 'quickstart', 'Quickstart'],
    ['docs/installation.md', 'installation', 'Installation'],
    ['docs/guide/supported-data.md', 'supported-data', 'Supported data'],
    ['docs/guide/earthdata-login.md', 'earthdata-login', 'Earthdata login'],
  ] },
  { text: 'Get data', pages: [
    ['docs/guide/sentinel-2.md', 'sentinel-2', 'Sentinel-2'],
    ['docs/guide/landsat-hls-opera.md', 'landsat-hls-opera', 'Landsat, HLS and OPERA'],
    ['docs/guide/goes-smap-nisar.md', 'goes-smap-nisar', 'GOES, SMAP and NISAR'],
    ['docs/guide/swot.md', 'swot-raster', 'SWOT Raster'],
    ['docs/guide/zarr.md', 'zarr-stores', 'ERA5, IMERG and GFS (Zarr)'],
  ] },
  { text: 'Use the output', pages: [
    ['docs/guide/output.md', 'reading-output', 'Output, values and masks'],
    ['docs/guide/cli.md', 'cli-reference', 'CLI reference'],
    ['docs/native-http/cli.md', 'cli', 'Reader selection and Rust API'],
  ] },
  { text: 'Limits and evidence', pages: [
    ['docs/alpha/contract.md', 'contract', 'Support contract'],
    ['docs/products/roadmap.md', 'coverage', 'Roadmap and candidates'],
    ['docs/products/specifications.md', 'science', 'Landsat, HLS and OPERA definitions'],
    ['docs/alternatives.md', 'alternatives', 'Alternatives and measurements'],
    ['docs/consolidation/validation.md', 'validation', 'Validation'],
    ['docs/alpha/licensing-closeout.md', 'licensing', 'License and notices'],
    ['docs/release-preparation.md', 'release', 'Release preparation'],
  ] },
  { text: 'Detailed workflows and reports', collapsed: true, pages: [
    ['docs/products/workflow.md', 'products', 'Named products: workflow and evidence'],
    ['docs/native-sentinel/workflow.md', 'sentinel', 'Sentinel-2 native workflow'],
    ['docs/scientific-output/workflow.md', 'output', 'Sentinel-2 Zarr output details'],
    ['docs/products/wave3/qualification.md', 'arrays', 'GOES, SMAP and NISAR report'],
    ['docs/products/wave4/qualification.md', 'geography', 'Geographic array selection report'],
    ['docs/swot-job/workflow.md', 'swot', 'SWOT jobs: full contract'],
    ['docs/swot-aoi/workflow.md', 'swot-geography', 'SWOT geographic selection: full contract'],
    ['docs/zarr-v3-input/workflow.md', 'zarr', 'Zarr v3 input profile'],
    ['docs/providers/qualification.md', 'providers', 'Provider routing report'],
    ['docs/transfer-resume/workflow.md', 'resume', 'SWOT transfer resume'],
  ] },
];
export const pages = sections.flatMap(section => section.pages);
