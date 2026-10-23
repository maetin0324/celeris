/** Reject unrelated loopback probes before a fake daemon records or asserts a request.
 * Body-consuming handlers still wait for end; streaming-upload tests deliberately observe aborts.
 * Keep auth assertions in the handler so a broken gateway token is never silently ignored.
 */
export function acceptFixtureRequest(req, res, expected = (request) => request.url?.startsWith("/api/v1/")) {
  const unexpectedBody =
    ["GET", "HEAD"].includes(req.method) &&
    (req.headers["transfer-encoding"] || Number(req.headers["content-length"] ?? 0) !== 0);
  if (expected(req) && !unexpectedBody) return true;
  res.writeHead(404);
  res.end();
  return false;
}
