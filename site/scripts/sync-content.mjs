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

// Replaces the first H1 with the nav title and demotes any later H1 to H2.
// Lines inside fenced code blocks (shell comments start with "# ") are left alone.
function retitle(text, title) {
  let fence = null;
  let titled = false;
  return text.split('\n').map(line => {
    const marker = line.match(/^\s*(`{3,}|~{3,})(.*)$/);
    if (fence) {
      if (marker && marker[1][0] === fence[0] && marker[1].length >= fence.length && !marker[2].trim()) fence = null;
      return line;
    }
    if (marker) {
      fence = marker[1];
      return line;
    }
    if (!line.startsWith('# ')) return line;
    if (titled) return '#' + line;
    titled = true;
    return '# ' + title;
  }).join('\n');
}

for (const [source, route, title] of pages) {
  const original = retitle(readFileSync(resolve(root, source), 'utf8'), title);
  const raw = rewrite(original, source, true);
  writeFileSync(resolve(site, 'docs/public/raw', route + '.md'), raw);
  const content = rewrite(original, source)
    .split(/(```[\s\S]*?```|~~~[\s\S]*?~~~|`+[^`]*`+)/g)
    .map(part => part.startsWith('`') || part.startsWith('~~~') ? part
      : part.replace(/<(https?:\/\/[^>\s]+)>/g, '[$1]($1)')
        .replaceAll('<', '&lt;').replaceAll('{', '&#123;').replaceAll('}', '&#125;'))
    .join('');
  writeFileSync(resolve(site, 'docs/pages', route + '.md'),
    `---\ntitle: ${JSON.stringify(title)}\n---\n\n` + content);
}
writeFileSync(resolve(site, 'docs/public/llms.txt'), '# Tile-Ripper\n\nNative scientific raster acquisition in Rust.\n\n' +
  pages.map(([, route, title]) => `- [${title}](${origin}/raw/${route}.md)`).join('\n') + '\n');
console.log(`Synced ${pages.length} source documents without changing their originals.`);
