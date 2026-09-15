import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const releases = 'https://github.com/aihenryai/hebrew-dictation/releases';
export const manifestUrl = `${releases}/latest/download/latest.json`;

// Read-only publication check: no credentials, installation, or asset downloads.
// Signature comparison checks publication consistency, not cryptographic validity.
export async function checkRelease(version, request = fetch) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Expected a stable x.y.z version');
  const base = `${releases}/download/v${version}/hebrew-dictation-v${version}`;
  const expected = {
    'windows-x86_64': `${base}-x64.exe`,
    'windows-x86_64-nsis': `${base}-x64.exe`,
    'darwin-aarch64': `${base}-aarch64.app.tar.gz`,
    'darwin-aarch64-app': `${base}-aarch64.app.tar.gz`,
  };
  async function get(url, method = 'GET') {
    const response = await request(url, { method, signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`${method} ${url}: HTTP ${response.status}`);
    return response;
  }
  const manifest = await (await get(manifestUrl)).json();
  if (manifest.version !== version) throw new Error(`Latest version is ${manifest.version}, expected ${version}`);
  for (const name of ['windows-x86_64', 'darwin-aarch64']) {
    if (!manifest.platforms?.[name]) throw new Error(`Missing platform: ${name}`);
  }
  const verified = new Map();
  async function checkBinary(url) {
    const response = await get(url, 'HEAD');
    const type = response.headers.get('content-type') || '';
    if (!/^(application\/octet-stream|application\/x-[^;]+|application\/gzip|binary\/octet-stream)(;|$)/i.test(type)) {
      throw new Error(`Not a binary download: ${url} (${type})`);
    }
    const size = Number(response.headers.get('content-length'));
    if (!(size > 0)) throw new Error(`Missing or empty download: ${url}`);
  }
  for (const [name, platform] of Object.entries(manifest.platforms)) {
    if (!expected[name] || platform.url !== expected[name]) throw new Error(`Unexpected download URL for ${name}`);
    if (typeof platform.signature !== 'string' || !platform.signature.trim()) throw new Error(`Missing signature: ${name}`);
    if (!verified.has(platform.url)) {
      await checkBinary(platform.url);
      const signature = (await (await get(`${platform.url}.sig`)).text()).trim();
      verified.set(platform.url, signature);
    }
    if (verified.get(platform.url) !== platform.signature.trim()) throw new Error(`Signature mismatch: ${name}`);
  }
  await checkBinary(`${base}-aarch64.dmg`);
  return { version, platforms: Object.keys(manifest.platforms), downloads: verified.size + 1 };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const version = process.argv[2] ?? JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')).version;
  try {
    const result = await checkRelease(version);
    console.log(`Release ${result.version}: ${result.platforms.length} updater entries, ${result.downloads} downloads verified.`);
    console.log('Published signature files match the updater manifest. Installation/signature cryptography was not tested.');
  } catch (error) {
    console.error(`Release check failed: ${error.message}`);
    process.exitCode = 1;
  }
}
