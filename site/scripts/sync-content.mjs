import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, existsSync, statSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { origin, pages, repository } from '../content.mjs';

const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(site, '..');
const targets = new Map(pages.map(([source, route]) => [source, '/' + route]));
mkdirSync(resolve(site, 'docs/pages'), { recursive: true });
for (const name of readdirSync(resolve(site, 'docs/pages'))) {
  if (name.endsWith('.md')) rmSync(resolve(site, 'docs/pages', name));
}
rmSync(resolve(site, 'docs/public/raw'), { recursive: true, force: true });
mkdirSync(resolve(site, 'docs/public/raw'), { recursive: true });

function rewrite(text, source, raw = false) {
  return text.replace(/(!?)\[([^\]]*)\]\(([^)]+)\)/g, (match, image, label, href) => {
    if (/^(?:https?:|mailto:|#)/.test(href)) return match;
    const [file, fragment] = href.split('#');
    const path = relative(root, resolve(root, dirname(source), file));
    if (!existsSync(resolve(root, path))) throw new Error(`Missing source link ${source}: ${href}`);
    const local = targets.get(path);
    const url = local ? (raw ? origin + '/raw' + local + '.md' : local)
      : `${repository}/${statSync(resolve(root, path)).isDirectory() ? 'tree' : 'blob'}/main/${path}`;
    return `${image}[${label}](${url}${fragment ? '#' + fragment : ''})`;
  });
}

for (const [source, route, title] of pages) {
  const original = readFileSync(resolve(root, source), 'utf8').replace(/^# .+$/m, '# ' + title);
  const raw = rewrite(original, source, true);
  writeFileSync(resolve(site, 'docs/public/raw', route + '.md'), raw);
  const content = rewrite(original, source)
    .split(/(```[\s\S]*?```|~~~[\s\S]*?~~~|`+[^`]*`+)/g)
    .map(part => part.startsWith('`') || part.startsWith('~~~') ? part
      : part.replaceAll('<', '&lt;').replaceAll('{', '&#123;').replaceAll('}', '&#125;'))
    .join('');
  writeFileSync(resolve(site, 'docs/pages', route + '.md'),
    `---\ntitle: ${JSON.stringify(title)}\n---\n\n` + content);
}
writeFileSync(resolve(site, 'docs/public/llms.txt'), '# Tile-Ripper\n\nNative scientific raster acquisition in Rust.\n\n' +
  pages.map(([, route, title]) => `- [${title}](${origin}/raw/${route}.md)`).join('\n') + '\n');
console.log(`Synced ${pages.length} source documents without changing their originals.`);
