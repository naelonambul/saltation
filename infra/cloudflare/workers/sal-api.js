export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    url.protocol = "http:";
    url.hostname = "127.0.0.1";
    url.port = "50081";
    const headers = new Headers(request.headers);
    const ip = request.headers.get("CF-Connecting-IP");
    if (ip) headers.set("CF-Connecting-IP", ip);
    const country = request.cf?.country || request.headers.get("CF-IPCountry");
    if (country) headers.set("CF-IPCountry", country);
    return env.SAL_API.fetch(
      new Request(url.toString(), {
        method: request.method,
        headers,
        body: request.body,
        redirect: "manual",
      })
    );
  },
};
