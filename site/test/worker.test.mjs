import assert from 'node:assert/strict';
import test from 'node:test';
import worker from '../worker.js';

test('www redirects to the apex while retaining path and query', async () => {
  const response = await worker.fetch(new Request('https://www.tileripper.com/quickstart?x=1'), {});
  assert.equal(response.status, 301);
  assert.equal(response.headers.get('Location'), 'https://tileripper.com/quickstart?x=1');
});

test('apex delegates missing and existing assets without an SPA success fallback', async () => {
  for (const status of [200, 404]) {
    const response = await worker.fetch(new Request('https://tileripper.com/example'), {
      ASSETS: { fetch: async request => new Response(new URL(request.url).pathname, { status }) },
    });
    assert.equal(response.status, status);
    assert.equal(await response.text(), '/example');
  }
});
