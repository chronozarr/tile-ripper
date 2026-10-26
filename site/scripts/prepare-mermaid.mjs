// Adapted from chronozarr/chronozarr site/scripts/prepare-mermaid.mjs; see site/README.md.
// mermaid-isomorphic loads Mermaid's classic prebuilt bundle. That file embeds old
// DOMPurify/KaTeX independently of the installed packages, so npm overrides alone
// cannot patch it. Rebuild the public core entry with the audited dependencies.
import { build } from 'esbuild';
import { readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const lock = JSON.parse(await readFile(resolve(site, 'package-lock.json'), 'utf8'));
const expected = { mermaid: '11.17.2', dompurify: '3.4.17', katex: '0.18.2' };
const entries = new Set();
const slash = path => path.replaceAll('\\', '/');

// Follow every installed renderer's own resolution, including nested copies.
for (const path of Object.keys(lock.packages)) {
  if (!path.endsWith('/mermaid-isomorphic')) continue;
  const renderer = resolve(site, path);
  const pkg = JSON.parse(await readFile(resolve(renderer, 'package.json'), 'utf8'));
  if (pkg.version !== '3.1.0') throw new Error(`review Mermaid loader contract for ${pkg.version}`);
  const source = await readFile(resolve(renderer, 'dist/mermaid-isomorphic.js'), 'utf8');
  if (!source.includes("import.meta.resolve('mermaid/dist/mermaid.js')")) {
    throw new Error('mermaid-isomorphic changed its browser entry; review preparation');
  }
  const require = createRequire(resolve(renderer, 'package.json'));
  const core = require.resolve('mermaid');
  if (!slash(core).endsWith('/dist/mermaid.core.mjs') ||
      require.resolve('mermaid/dist/mermaid.js') !== resolve(dirname(core), 'mermaid.js')) {
    throw new Error('Mermaid public core/classic paths changed; review preparation');
  }
  entries.add(core);
}
if (!entries.size) throw new Error('no installed mermaid-isomorphic renderer found');

for (const core of entries) {
  const pkg = JSON.parse(await readFile(resolve(dirname(core), '../package.json'), 'utf8'));
  if (pkg.version !== expected.mermaid) throw new Error(`review Mermaid core ${pkg.version}`);
  const result = await build({
    absWorkingDir: site,
    stdin: {
      contents: `import mermaid from ${JSON.stringify(core)}; globalThis.mermaid = mermaid;`,
      resolveDir: site,
      sourcefile: 'tile-ripper-mermaid-entry.js',
    },
    bundle: true,
    format: 'iife',
    platform: 'browser',
    target: 'es2020',
    legalComments: 'inline',
    metafile: true,
    write: false,
  });
  const inputs = Object.keys(result.metafile.inputs).map(path => resolve(site, path));
  for (const name of ['dompurify', 'katex']) {
    const roots = new Set(inputs.flatMap(path => {
      const marker = `/node_modules/${name}/`;
      const index = slash(path).lastIndexOf(marker);
      return index < 0 ? [] : [path.slice(0, index + marker.length)];
    }));
    if (!roots.size) throw new Error(`Mermaid bundle no longer imports installed ${name}`);
    for (const root of roots) {
      const dependency = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
      if (dependency.version !== expected[name]) {
        throw new Error(`unreviewed bundled ${name} ${dependency.version}; expected ${expected[name]}`);
      }
    }
  }
  if (!inputs.includes(core) || inputs.some(path => /\/mermaid\/dist\/mermaid\.(?:min\.)?(?:js|esm(?:\.min)?\.mjs)$/.test(slash(path)))) {
    throw new Error('Mermaid preparation used a prebundled entry instead of core');
  }
  const output = resolve(dirname(core), 'mermaid.js');
  await writeFile(output, result.outputFiles[0].contents);
  console.log(`Prepared ${output}: Mermaid ${pkg.version}, DOMPurify ${expected.dompurify}, KaTeX ${expected.katex}.`);
}
