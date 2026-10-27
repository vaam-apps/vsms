/**
 * Which operation of the schema a concrete path is, from the table the signature
 * binds (`sdks/rust/vsms-sdk-wasm`'s `OP_CONTRACTS`).
 *
 * A server that carries sealed requests for someone else (the operator console's
 * pass-through) asks this before it opens a connection: the gateway seals
 * exactly the routes in this table, so a path that is not in it is not the
 * pipe's to carry.
 */
import { isWasmFailure, loadWasm } from "./wasm.js";

/**
 * The route template and decoded parameters of `method path` (a path with no
 * query, percent-encoding as received), or `undefined` when the schema serves
 * no such operation.
 *
 * @example
 * routeOf("GET", "/messages/m%2F1"); // { route: "/messages/{id}", params: ["m/1"] }
 * routeOf("GET", "/healthz"); // undefined
 */
export function routeOf(
  method: string,
  path: string,
): { route: string; params: string[] } | undefined {
  try {
    return loadWasm().routeOf(method, path);
  } catch (err) {
    if (isWasmFailure(err) && err.kind === "unbound") return undefined;
    throw err;
  }
}
