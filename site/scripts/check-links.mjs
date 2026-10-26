import { readdirSync, readFileSync, statSync, existsSync, writeFileSync } from 'node:fs';
import { resolve, relative, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../docs/dist');
function files(directory) {
  return readdirSync(directory).flatMap(name => {
    const path = resolve(directory, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}
const html = files(root).filter(path => path.endsWith('.html'));
const missing = [];
for (const page of html) {
  const base = 'https://tileripper.com/' + relative(root, page).replace(/(?:index)?\.html$/, '');
  for (const [, href] of readFileSync(page, 'utf8').matchAll(/(?:href|src)="([^"<>]+)"/g)) {
    if (/^(?:data:|mailto:|javascript:)/.test(href)) continue;
    const url = new URL(href.replaceAll('&amp;', '&'), base);
    if (url.origin !== 'https://tileripper.com') continue;
    const path = resolve(root, '.' + decodeURIComponent(url.pathname));
    const target = [path, path + '.html', resolve(path, 'index.html')]
      .find(candidate => existsSync(candidate) && statSync(candidate).isFile());
    if (!target) missing.push(`${relative(root, page)}: ${href}`);
    else if (url.hash && target.endsWith('.html')) {
      const id = decodeURIComponent(url.hash.slice(1));
      const text = readFileSync(target, 'utf8');
      if (id && !text.includes(`id="${id}"`)) missing.push(`${relative(root, page)}: missing anchor ${href}`);
    }
  }
}
if (missing.length) throw new Error(missing.join('\n'));
const locations = html.filter(path => !path.endsWith('404.html')).map(path =>
  'https://tileripper.com/' + relative(root, path).replace(/(?:index)?\.html$/, ''));
writeFileSync(resolve(root, 'sitemap.xml'), '<?xml version="1.0" encoding="UTF-8"?>\n' +
  '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">' +
  locations.map(location => '<url><loc>' + location + '</loc></url>').join('') + '</urlset>\n');
console.log(`Checked local links and anchors in ${html.length} HTML pages.`);
