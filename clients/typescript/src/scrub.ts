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
