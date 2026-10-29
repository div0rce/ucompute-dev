// OpenAI JS client smoke test against `ucompute serve` (F-09-01).
// Usage: node openai_client_smoke.mjs BASE_URL MODEL_ID OPENAI_JS_DIR [API_KEY]
// OPENAI_JS_DIR holds a node_modules with the `openai` package (examples/clients/openai-js). Only
// baseURL (and optionally the key) differ from talking to api.openai.com. Prints
// "ALL CHECKS PASSED" and exits 0 when every check holds; any failure throws (exit 1).
import { createRequire } from "node:module";
import path from "node:path";

const [baseURL, model, jsDir, apiKey = "not-needed"] = process.argv.slice(2);
const { default: OpenAI, NotFoundError, BadRequestError } = createRequire(path.join(jsDir, "/"))("openai");
const client = new OpenAI({ baseURL, apiKey, maxRetries: 0 });
const PROMPT = "Once upon a time, a little rabbit";
const GOLDEN = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";
const MESSAGES = [{ role: "user", content: "Tell me a story." }];
const assert = (cond, msg) => { if (!cond) throw new Error(msg); };

const ids = [];
for await (const m of client.models.list()) ids.push(m.id);
assert(ids.includes(model), `model ${model} not listed: ${ids.join(", ")}`);
assert((await client.models.retrieve(model)).id === model, "retrieve");
console.log("models.list / retrieve ok");

let r = await client.completions.create({ model, prompt: PROMPT, max_tokens: 32, temperature: 0, seed: 42 });
assert(r.choices[0].text === GOLDEN, `golden mismatch: ${JSON.stringify(r.choices[0].text)}`);
assert(r.usage.prompt_tokens === 10 && r.usage.completion_tokens === 32, JSON.stringify(r.usage));
assert(r.choices[0].finish_reason === "length", r.choices[0].finish_reason);
console.log("completions.create golden text ok");

r = await client.chat.completions.create({ model, messages: MESSAGES, max_tokens: 8, temperature: 0 });
assert(r.choices[0].message.role === "assistant" && r.choices[0].message.content, "chat content");
assert(r.usage.completion_tokens === 8 && r.id.startsWith("chatcmpl-"), "chat usage/id");
console.log("chat.completions.create ok");

// A tools request round-trips through the client's tool-call parsing (the fixture model has no
// chat template, so no call is produced; the typed `tool_calls` field must still parse).
const tools = [{ type: "function", function: { name: "get_weather", description: "Weather for a city", parameters: { type: "object", properties: { city: { type: "string" } }, required: ["city"] } } }];
r = await client.chat.completions.create({ model, messages: [{ role: "user", content: "Weather in Paris?" }], max_tokens: 8, temperature: 0, tools });
assert(r.choices[0].message.tool_calls === undefined || Array.isArray(r.choices[0].message.tool_calls), "tool_calls shape");
assert(["stop", "length", "tool_calls"].includes(r.choices[0].finish_reason), r.choices[0].finish_reason);
console.log("chat.completions.create(tools) ok:", JSON.stringify(r.choices[0].message.tool_calls ?? null));

const chunks = [];
const stream = await client.chat.completions.create({ model, messages: MESSAGES, max_tokens: 8, temperature: 0, stream: true, stream_options: { include_usage: true } });
for await (const chunk of stream) chunks.push(chunk);
const tokenChunks = chunks.filter((c) => c.choices?.[0]?.delta?.content);
assert(tokenChunks.length > 1, `only ${tokenChunks.length} content chunks`);
assert(chunks.some((c) => c.choices?.[0]?.finish_reason === "length"), "no finish chunk");
assert(chunks.at(-1).usage?.completion_tokens === 8, "usage chunk");
assert(chunks.every((c) => c.id === chunks[0].id), "chunk ids");
console.log(`chat.completions.create(stream) ok: ${tokenChunks.length} content chunks`);

try { await client.chat.completions.create({ model: "no-such-model", messages: MESSAGES, max_tokens: 1 }); assert(false, "expected NotFoundError"); }
catch (e) { assert(e instanceof NotFoundError && e.status === 404 && e.code === "model_not_found", `typed 404: ${e}`); console.log("typed 404 for an unknown model ok"); }
try { await client.chat.completions.create({ model, messages: [], max_tokens: 1 }); assert(false, "expected BadRequestError"); }
catch (e) { assert(e instanceof BadRequestError && e.code === "invalid_request", `typed 400: ${e}`); console.log("typed 400 for an invalid request ok"); }

console.log("ALL CHECKS PASSED");
