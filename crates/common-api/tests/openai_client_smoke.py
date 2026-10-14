"""OpenAI Python client smoke test against `common serve` (F-09-01).

Usage: openai_client_smoke.py BASE_URL MODEL_ID [API_KEY]

Only `base_url` (and optionally the key) differ from talking to api.openai.com. Exits 0 and prints
"ALL CHECKS PASSED" when every check holds; any failure raises and exits non-zero. The golden text
is the greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609.
"""

import sys

import openai
from openai import OpenAI

PROMPT = "Once upon a time, a little rabbit"
GOLDEN = (
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it."
    " But the radish was too big"
)
MESSAGES = [{"role": "user", "content": "Tell me a story."}]


def main() -> None:
    base_url, model = sys.argv[1], sys.argv[2]
    api_key = sys.argv[3] if len(sys.argv) > 3 else "not-needed"
    client = OpenAI(base_url=base_url, api_key=api_key, max_retries=0)

    ids = [m.id for m in client.models.list()]
    assert model in ids, ids
    assert client.models.retrieve(model).id == model
    print("models.list / retrieve ok")

    r = client.completions.create(model=model, prompt=PROMPT, max_tokens=32, temperature=0, seed=42)
    assert r.choices[0].text == GOLDEN, repr(r.choices[0].text)
    assert (r.usage.prompt_tokens, r.usage.completion_tokens) == (10, 32), r.usage
    assert r.choices[0].finish_reason == "length"
    print("completions.create golden text ok")

    r = client.chat.completions.create(model=model, messages=MESSAGES, max_tokens=8, temperature=0)
    assert r.choices[0].message.role == "assistant" and r.choices[0].message.content, r
    assert r.choices[0].finish_reason == "length"
    assert r.usage.completion_tokens == 8, r.usage
    assert r.id.startswith("chatcmpl-")
    print("chat.completions.create ok")

    # A tools request round-trips through the client's tool-call parsing (the fixture model has no
    # chat template, so no call is produced; the typed `tool_calls` field must still parse).
    tools = [{"type": "function", "function": {"name": "get_weather", "description": "Weather for a city",
              "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}}]
    r = client.chat.completions.create(
        model=model, messages=[{"role": "user", "content": "Weather in Paris?"}], max_tokens=8, temperature=0, tools=tools,
    )
    assert r.choices[0].message.tool_calls is None or isinstance(r.choices[0].message.tool_calls, list), r
    assert r.choices[0].finish_reason in ("stop", "length", "tool_calls"), r
    print(f"chat.completions.create(tools) ok: tool_calls={r.choices[0].message.tool_calls!r}")

    chunks = list(
        client.chat.completions.create(
            model=model, messages=MESSAGES, max_tokens=8, temperature=0, stream=True,
            stream_options={"include_usage": True},
        )
    )
    token_chunks = [c for c in chunks if c.choices and c.choices[0].delta.content]
    assert len(token_chunks) > 1, len(token_chunks)
    assert "".join(c.choices[0].delta.content for c in token_chunks)
    assert any(c.choices and c.choices[0].finish_reason == "length" for c in chunks), chunks
    assert chunks[-1].usage is not None and chunks[-1].usage.completion_tokens == 8, chunks[-1]
    assert all(c.id == chunks[0].id for c in chunks)
    print(f"chat.completions.create(stream=True) ok: {len(token_chunks)} content chunks")

    try:
        client.chat.completions.create(model="no-such-model", messages=MESSAGES, max_tokens=1)
    except openai.NotFoundError as e:
        assert e.status_code == 404, e
        assert e.code == "model_not_found", e.body
        print("typed 404 for an unknown model ok")
    else:
        raise AssertionError("unknown model did not raise NotFoundError")

    try:
        client.chat.completions.create(model=model, messages=[], max_tokens=1)
    except openai.BadRequestError as e:
        assert e.code == "invalid_request", e.body
        print("typed 400 for an invalid request ok")
    else:
        raise AssertionError("empty messages did not raise BadRequestError")

    print("ALL CHECKS PASSED")


if __name__ == "__main__":
    main()
