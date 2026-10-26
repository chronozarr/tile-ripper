import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const lock = JSON.parse(readFileSync(resolve(site, 'package-lock.json'), 'utf8'));
const texts = new Map();
const rows = [];
for (const [path, metadata] of Object.entries(lock.packages)) {
  if (!path || !existsSync(resolve(site, path, 'package.json'))) continue;
  const pkg = JSON.parse(readFileSync(resolve(site, path, 'package.json'), 'utf8'));
  const notices = [];
  for (const name of readdirSync(resolve(site, path))) {
    const file = resolve(site, path, name);
    if (!/^(?:license|licence|copying|copyright|notice|authors)/i.test(name) || !statSync(file).isFile()) continue;
    const bytes = readFileSync(file);
    const hash = createHash('sha256').update(bytes).digest('hex');
    texts.set(hash, bytes.toString('utf8'));
    notices.push(hash);
  }
  rows.push({ package: `${pkg.name}@${pkg.version}`, declaredLicense: pkg.license ?? metadata.license ?? null, notices });
}
writeFileSync(resolve(site, 'docs/public/third-party-notices.txt'),
  'Tile-Ripper documentation build dependency notice evidence\n' +
  'Includes installed build-time and optional packages, not a claim that all are shipped to browsers.\n\n' +
  JSON.stringify(rows, null, 2) + '\n\n' + [...texts].map(([hash, text]) => `SHA-256 ${hash}\n${text}\n`).join('\n'));
console.log(`Collected ${texts.size} original notice texts for ${rows.length} installed site packages.`);
