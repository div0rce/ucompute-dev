// Keeps the TypeScript error table in sync with crates/ucompute-core/src/error.rs (F-10-06).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ApiError, UNREACHABLE } from "./client";
import { ERROR_CODES, explain, mapError } from "./errors";

const rs = readFileSync(join(__dirname, "../../../../crates/ucompute-core/src/error.rs"), "utf8");

/** Variants listed in `ErrorCode::ALL`. */
function variantsInAll(): string[] {
  const block = rs.match(/pub const ALL[^=]*=\s*&\[([\s\S]*?)\];/);
  if (!block) throw new Error("ErrorCode::ALL not found in error.rs");
  return [...block[1].matchAll(/ErrorCode::(\w+)/g)].map((m) => m[1]);
}

/** Variant -> snake_case string from `as_str`. */
function asStr(): Record<string, string> {
  return Object.fromEntries([...rs.matchAll(/ErrorCode::(\w+)\s*=>\s*"([a-z_]+)"/g)].map((m) => [m[1], m[2]]));
}

/** Variant -> (title, next_step) from `user_message`. */
function userMessages(): Record<string, [string, string]> {
  return Object.fromEntries([...rs.matchAll(/ErrorCode::(\w+)\s*=>\s*\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)/g)].map((m) => [m[1], [m[2], m[3]]]));
}

describe("error code table matches error.rs", () => {
  const all = variantsInAll();
  const codes = asStr();
  const messages = userMessages();

  it("parsed the Rust source", () => {
    expect(all.length).toBeGreaterThan(10);
    expect(Object.keys(codes).length).toBe(all.length);
    expect(Object.keys(messages).length).toBe(all.length);
  });

  it.each(all)("ErrorCode::%s has a TypeScript entry with the same text", (variant) => {
    const code = codes[variant];
    expect(code, `as_str for ${variant}`).toBeTruthy();
    const entry = ERROR_CODES[code];
    expect(entry, `ERROR_CODES.${code}`).toBeDefined();
    expect(entry.title).toBe(messages[variant][0]);
    expect(entry.next_step).toBe(messages[variant][1]);
    expect(entry.title.length).toBeGreaterThan(0);
    expect(entry.next_step.length).toBeGreaterThan(0);
  });

  it("has no TypeScript entries the Rust side does not know", () => {
    const known = new Set(Object.values(codes));
    for (const k of Object.keys(ERROR_CODES)) expect(known.has(k), k).toBe(true);
  });
});

describe("mapError", () => {
  it("maps a known code to its plain-language explanation", () => {
    const m = mapError("insufficient_memory", "need 9 GiB, have 4 GiB", "try Q4");
    expect(m.title).toBe(ERROR_CODES.insufficient_memory.title);
    expect(m.detail).toBe("need 9 GiB, have 4 GiB");
    expect(m.hint).toBe("try Q4");
  });
  it("falls back to internal for unknown codes", () => {
    expect(mapError("something_new", "x").title).toBe(ERROR_CODES.internal.title);
  });
  it("explains an unreachable daemon without a daemon code", () => {
    expect(explain(new ApiError(UNREACHABLE, "boom")).next_step).toMatch(/ucompute serve/);
  });
  it("explains plain Errors as internal", () => {
    expect(explain(new Error("nope")).code).toBe("internal");
  });
});
