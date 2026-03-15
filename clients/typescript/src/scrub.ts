export interface ScrubRule {
  pattern: RegExp;
  replacement: string;
}

const EMAIL = /[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}/g;

export function scrub(
  input: string | undefined,
  custom?: ScrubRule[]
): string | undefined {
  if (input === undefined) return undefined;
  let result = input.replace(EMAIL, "[EMAIL]");
  if (custom) {
    for (const rule of custom) {
      result = result.replace(rule.pattern, rule.replacement);
    }
  }
  return result;
}

/** Recursively scrub all string values in an object/array. */
export function scrubObject<T>(value: T, custom?: ScrubRule[]): T {
  if (typeof value === "string") return scrub(value, custom) as T;
  if (Array.isArray(value)) return value.map((v) => scrubObject(v, custom)) as T;
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      out[k] = scrubObject(v, custom);
    }
    return out as T;
  }
  return value;
}
