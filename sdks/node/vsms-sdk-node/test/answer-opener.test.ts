import { strict as assert } from "node:assert";
import { test } from "node:test";
import { AnswerOpener, routeOf, SdkError } from "../dist/index.js";
import { loadWasm } from "../dist/wasm.js";
import { loadResponderModule, newTestIdentity } from "../support/responder.ts";

// The console's composer forwards a browser's sealed request and must believe
// the gateway's answer only if it opens against those bytes. These tests seal
// with a client key, answer with the server half, and open as a forwarder that
// holds only the request's bytes.

async function forwarded(options: { answerSeed?: "server" | "stranger" } = {}) {
  const identity = newTestIdentity();
  const module = loadResponderModule();
  const responder = new module.Responder(identity.serverSeed, [identity.clientPublic], "vsms");
  const sealer = loadWasm().Sealer.fromSeed(
    identity.clientSeed,
    identity.serverPublic,
    identity.nextPublic,
  );
  const sealed = await sealer.sealCall(
    "POST",
    "/$procs/authorizeComposition",
    [],
    '{"args":{"operation":"send","inputSha256":"' + "0".repeat(64) + '"}}',
    undefined,
    undefined,
  );
  const opened = await responder.openRequest(
    "POST",
    sealed.target,
    undefined,
    sealed.headers,
    sealed.body,
  );
  const answering =
    options.answerSeed === "stranger"
      ? await new module.Responder(newTestIdentity().serverSeed, [identity.clientPublic], "vsms")
          .openRequest("POST", sealed.target, undefined, sealed.headers, sealed.body)
          .then((o) => o.sealResponse(200, '{"userId":"u1"}'))
      : await opened.sealResponse(200, '{"userId":"u1"}');
  return {
    identity,
    request: { method: "POST", target: sealed.target, body: sealed.body, headers: sealed.headers },
    answer: { status: 200, headers: answering.headers, body: answering.body },
  };
}

const kind = (expected: string) => (error: unknown) =>
  error instanceof SdkError && error.kind === expected;

test("an answer to forwarded bytes opens to its payload", async () => {
  const f = await forwarded();
  const opener = new AnswerOpener({
    serverKeys: { current: f.identity.serverPublic, next: f.identity.nextPublic },
  });
  assert.equal(await opener.open(f.request, f.answer), '{"userId":"u1"}');
});

test("an answer sealed by a key that is not pinned is unverified", async () => {
  const f = await forwarded({ answerSeed: "stranger" });
  const opener = new AnswerOpener({
    serverKeys: { current: f.identity.serverPublic, next: f.identity.nextPublic },
  });
  await assert.rejects(opener.open(f.request, f.answer), kind("unverified"));
});

test("a refusal presented as a success, or an answer to other bytes, is unverified", async () => {
  const f = await forwarded();
  const other = await forwarded();
  const opener = new AnswerOpener({
    serverKeys: { current: f.identity.serverPublic, next: f.identity.nextPublic },
  });
  await assert.rejects(opener.open(f.request, { ...f.answer, status: 403 }), kind("unverified"));
  await assert.rejects(opener.open(other.request, f.answer), kind("unverified"));
});

test("the gateway's unsigned 401 is unsigned and nothing in it is read", async () => {
  const f = await forwarded();
  const opener = new AnswerOpener({
    serverKeys: { current: f.identity.serverPublic, next: f.identity.nextPublic },
  });
  await assert.rejects(
    opener.open(f.request, {
      status: 401,
      headers: [["content-type", "text/plain"]],
      body: new TextEncoder().encode("request could not be authenticated"),
    }),
    (error: unknown) => kind("unsigned")(error) && (error as SdkError).httpStatus === 401,
  );
});

test("server keys are required and must be two distinct keys", () => {
  const identity = newTestIdentity();
  assert.throws(
    () => new AnswerOpener({ serverKeys: undefined as never }),
    (error: unknown) => kind("config")(error),
  );
  assert.throws(
    () =>
      new AnswerOpener({
        serverKeys: { current: identity.serverPublic, next: identity.serverPublic },
      }),
    (error: unknown) => kind("config")(error),
  );
});

// ---- the table a pass-through consults ---------------------------------

test("routeOf names the schema's route for a concrete path, and nothing for any other", () => {
  assert.deepEqual(routeOf("GET", "/messages/m%2F1"), {
    route: "/messages/{id}",
    params: ["m/1"],
  });
  assert.deepEqual(routeOf("POST", "/$procs/authorizeComposition"), {
    route: "/$procs/authorizeComposition",
    params: [],
  });
  for (const [method, path] of [
    ["GET", "/healthz"],
    ["GET", "/metrics"],
    ["POST", "/token"],
    ["POST", "/dlr/orange_cm"],
    ["GET", "/login"],
    ["DELETE", "/$procs/sendMessage"],
    ["GET", "/messages/.."],
  ] as const) {
    assert.equal(routeOf(method, path), undefined, `${method} ${path}`);
  }
});
