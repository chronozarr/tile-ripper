export const repository = 'https://github.com/chronozarr/tile-ripper';
export const origin = 'https://tileripper.com';
export const sections = [
  { text: 'Start here', pages: [
    ['docs/quickstart.md', 'quickstart', 'Quickstart'],
    ['docs/installation.md', 'installation', 'Installation'],
    ['docs/alpha/contract.md', 'contract', 'Supported profiles and limits'],
  ] },
  { text: 'Acquire data', pages: [
    ['docs/products/workflow.md', 'products', 'Named products'],
    ['docs/native-sentinel/workflow.md', 'sentinel', 'Sentinel-2'],
    ['docs/products/wave3/qualification.md', 'arrays', 'GOES, SMAP and NISAR'],
    ['docs/products/wave4/qualification.md', 'geography', 'Geographic array selection'],
    ['docs/swot-job/workflow.md', 'swot', 'SWOT jobs'],
    ['docs/swot-aoi/workflow.md', 'swot-geography', 'SWOT geographic selection'],
    ['docs/zarr-v3-input/workflow.md', 'zarr', 'Zarr v3 input'],
    ['docs/providers/qualification.md', 'providers', 'Providers and authentication'],
  ] },
  { text: 'Use the output', pages: [
    ['docs/scientific-output/workflow.md', 'output', 'Native output and xarray'],
    ['docs/native-http/cli.md', 'cli', 'CLI and Rust consumers'],
    ['docs/transfer-resume/workflow.md', 'resume', 'SWOT transport resume'],
  ] },
  { text: 'Evidence and development', pages: [
    ['docs/products/roadmap.md', 'coverage', 'Product coverage'],
    ['docs/products/specifications.md', 'science', 'Scientific definitions'],
    ['docs/alternatives.md', 'alternatives', 'Alternatives and measurements'],
    ['docs/consolidation/validation.md', 'validation', 'Validation'],
    ['docs/alpha/licensing-closeout.md', 'licensing', 'License and notices'],
  ] },
];
export const pages = sections.flatMap(section => section.pages);
