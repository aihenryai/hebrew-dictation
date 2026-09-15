import assert from 'node:assert/strict';
import { test } from 'node:test';
import { checkRelease, manifestUrl } from './check-release.mjs';

function fixture() {
  const base = 'https://github.com/aihenryai/hebrew-dictation/releases/download/v2.13.9/hebrew-dictation-v2.13.9';
  const windows = { url: `${base}-x64.exe`, signature: 'windows-signature' };
  const mac = { url: `${base}-aarch64.app.tar.gz`, signature: 'mac-signature' };
  const manifest = { version: '2.13.9', platforms: {
    'windows-x86_64': { ...windows }, 'windows-x86_64-nsis': { ...windows },
    'darwin-aarch64': { ...mac }, 'darwin-aarch64-app': { ...mac },
  } };
  const responses = new Map([
    [manifestUrl, () => Response.json(manifest)],
    ...[windows.url, mac.url, `${base}-aarch64.dmg`].map(url => [url, () => new Response(null, {
      headers: { 'content-type': 'application/octet-stream', 'content-length': '1000' },
    })]),
    [`${windows.url}.sig`, () => new Response('windows-signature\n')],
    [`${mac.url}.sig`, () => new Response('mac-signature\n')],
  ]);
  const calls = [];
  const request = async (url, options) => {
    calls.push([url, options.method]);
    assert.ok(options.signal instanceof AbortSignal);
    assert.ok(responses.has(url), `Unexpected network request: ${url}`);
    return responses.get(url)();
  };
  return { manifest, responses, calls, request, windows, mac, base };
}

test('checks both operating systems and DMG without downloading installers', async () => {
  const f = fixture();
  const result = await checkRelease('2.13.9', f.request);
  assert.equal(result.downloads, 3);
  assert.equal(result.platforms.length, 4);
  assert.equal(f.calls.filter(([, method]) => method === 'HEAD').length, 3);
  assert.equal(f.calls.length, 6); // Duplicate platform aliases reuse network checks.
});

const failures = [
  ['stale latest release', f => { f.manifest.version = '2.13.6'; }, /Latest version/],
  ['missing Mac platform', f => { delete f.manifest.platforms['darwin-aarch64']; }, /Missing platform/],
  ['empty platforms', f => { f.manifest.platforms = {}; }, /Missing platform/],
  ['API metadata URL', f => { f.manifest.platforms['windows-x86_64'].url = 'https://api.github.com/repos/a/b/releases/assets/1'; }, /Unexpected download URL/],
  ['wrong architecture', f => { f.manifest.platforms['darwin-aarch64'].url = `${f.base}-x64.app.tar.gz`; }, /Unexpected download URL/],
  ['empty signature', f => { f.manifest.platforms['windows-x86_64'].signature = ' '; }, /Missing signature/],
  ['mismatched alias signature', f => { f.manifest.platforms['windows-x86_64-nsis'].signature = 'wrong'; }, /Signature mismatch/],
  ['missing installer', f => { f.responses.set(f.windows.url, () => new Response(null, { status: 404 })); }, /HTTP 404/],
  ['JSON instead of installer', f => { f.responses.set(f.windows.url, () => Response.json({ id: 1 })); }, /Not a binary/],
  ['empty installer', f => { f.responses.set(f.windows.url, () => new Response(null, { headers: { 'content-type': 'application/octet-stream', 'content-length': '0' } })); }, /Missing or empty/],
  ['missing DMG', f => { f.responses.set(`${f.base}-aarch64.dmg`, () => new Response(null, { status: 404 })); }, /HTTP 404/],
  ['missing signature file', f => { f.responses.set(`${f.mac.url}.sig`, () => new Response(null, { status: 404 })); }, /HTTP 404/],
];
for (const [name, mutate, error] of failures) {
  test(`rejects ${name}`, async () => {
    const f = fixture();
    mutate(f);
    await assert.rejects(checkRelease('2.13.9', f.request), error);
  });
}

test('invalid version fails before accessing the network', async () => {
  await assert.rejects(checkRelease('../latest', () => assert.fail('network accessed')), /Expected a stable/);
});
