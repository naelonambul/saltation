export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    url.protocol = "http:";
    url.hostname = "127.0.0.1";
    url.port = "50080";
    return env.SAL.fetch(new Request(url.toString(), request));
  },
};
