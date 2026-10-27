/**
 * Why `SMS_API_URL` cannot be used as the console's address for the gateway, or
 * `undefined` when it can.
 *
 * It is an origin, not a base path. The console builds every gateway URL as
 * `new URL(<path the schema serves>, SMS_API_URL)`, and a leading-slash path
 * replaces whatever path the base had, so a prefix (`https://edge/vsms`) would
 * be dropped and every sealed request sent to the origin's root, where it is
 * signed for a route the edge does not serve. That is refused at boot instead.
 * A gateway behind a path prefix is reached by a hostname of its own.
 *
 * ```ts ignore
 * gatewayUrlProblem("https://sms-gateway:8080"); // undefined
 * gatewayUrlProblem("https://sms-gateway:8080/"); // undefined
 * gatewayUrlProblem("https://edge.example.com/vsms"); // a reason
 * ```
 */
export function gatewayUrlProblem(value: string): string | undefined {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "SMS_API_URL is not a URL";
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return "SMS_API_URL must be an http: or https: URL";
  }
  if (url.username !== "" || url.password !== "") {
    return "SMS_API_URL must not carry credentials: the sealed request is the only credential";
  }
  if (url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    return (
      "SMS_API_URL must be the gateway's origin with no path, query or fragment " +
      "(for example https://sms-gateway:8080): every request path replaces a base path, " +
      "so a prefix would be dropped and requests sent to the origin's root"
    );
  }
  return undefined;
}
