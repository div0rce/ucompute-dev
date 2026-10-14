// Runs the unmodified official OpenAI JS client against `common serve` (only base_url differs).
// Usage: COMMON_BASE_URL=http://127.0.0.1:8192/v1 COMMON_MODEL=stories15M-q4_0 node smoke.mjs
import OpenAI from "openai";

const baseURL = process.env.COMMON_BASE_URL ?? "http://127.0.0.1:8192/v1";
const model = process.env.COMMON_MODEL ?? "stories15M-q4_0";
const client = new OpenAI({ baseURL, apiKey: process.env.COMMON_API_KEY ?? "not-needed-on-loopback" });

const models = await client.models.list();
const ids = [];
for await (const m of models) ids.push(m.id);
if (!ids.includes(model)) throw new Error(`model ${model} not listed; got ${ids.join(", ")}`);
console.log("models.list ok:", ids.length, "models");

const c = await client.completions.create({ model, prompt: "Once upon a time, a little rabbit", max_tokens: 32, temperature: 0, seed: 42 });
const text = c.choices[0].text;
if (!text.startsWith(" named Benny was hopping")) throw new Error(`unexpected completion: ${JSON.stringify(text)}`);
console.log("completions.create golden ok:", JSON.stringify(text.slice(0, 40)));

const chat = await client.chat.completions.create({ model, messages: [{ role: "user", content: "Once upon a time, a little rabbit" }], max_tokens: 16, temperature: 0 });
if (!chat.choices[0].message.content) throw new Error("empty chat content");
console.log("chat.completions.create ok:", chat.usage);

let chunks = 0, done = false;
const stream = await client.chat.completions.create({ model, messages: [{ role: "user", content: "Once upon a time, a little rabbit" }], max_tokens: 16, temperature: 0, stream: true });
for await (const chunk of stream) { if (chunk.choices?.[0]?.delta?.content) chunks++; if (chunk.choices?.[0]?.finish_reason) done = true; }
if (chunks < 2 || !done) throw new Error(`streaming: ${chunks} chunks, finish=${done}`);
console.log("chat.completions.create(stream) ok:", chunks, "chunks");

try { await client.chat.completions.create({ model: "no-such-model", messages: [{ role: "user", content: "x" }] }); throw new Error("expected 404"); }
catch (e) { if (e.status !== 404) throw e; console.log("typed 404 ok:", e.code ?? e.error?.code); }
console.log("ALL CHECKS PASSED");
