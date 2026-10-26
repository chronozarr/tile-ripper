export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname === 'www.tileripper.com') {
      url.hostname = 'tileripper.com';
      return Response.redirect(url, 301);
    }
    return env.ASSETS.fetch(request);
  },
};
