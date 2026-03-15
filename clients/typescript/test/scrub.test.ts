import { describe, it, expect } from "vitest";
import { scrub, scrubObject } from "../src/scrub";

describe("scrub", () => {
  it("redacts email addresses", () => {
    const input = "User alice@example.com failed login";
    expect(scrub(input)).toBe("User [EMAIL] failed login");
  });

  it("redacts multiple emails", () => {
    const input = "From bob@test.org to alice@example.com";
    expect(scrub(input)).toBe("From [EMAIL] to [EMAIL]");
  });

  it("leaves non-PII strings unchanged", () => {
    expect(scrub("NullPointerException at line 42")).toBe(
      "NullPointerException at line 42"
    );
  });

  it("redacts emails in stack traces", () => {
    const stack = `Error: auth failed for user@domain.com
    at login (/app/auth.ts:10)
    at handler (/app/api.ts:5)`;
    expect(scrub(stack)).toContain("[EMAIL]");
    expect(scrub(stack)).not.toContain("user@domain.com");
    expect(scrub(stack)).toContain("at login (/app/auth.ts:10)");
  });

  it("applies custom scrubbers", () => {
    const ssn = /\b\d{3}-\d{2}-\d{4}\b/g;
    const input = "SSN: 123-45-6789 failed";
    expect(scrub(input, [{ pattern: ssn, replacement: "[SSN]" }])).toBe(
      "SSN: [SSN] failed"
    );
  });

  it("applies custom scrubbers alongside email redaction", () => {
    const phone = /\b\d{3}-\d{3}-\d{4}\b/g;
    const input = "Contact alice@test.com or 555-123-4567";
    const result = scrub(input, [
      { pattern: phone, replacement: "[PHONE]" },
    ]);
    expect(result).toBe("Contact [EMAIL] or [PHONE]");
  });

  it("handles empty string", () => {
    expect(scrub("")).toBe("");
  });

  it("handles undefined", () => {
    expect(scrub(undefined)).toBeUndefined();
  });
});

describe("scrubObject", () => {
  it("scrubs string values in flat objects", () => {
    const input = { user: "alice@example.com", code: 500 };
    expect(scrubObject(input)).toEqual({ user: "[EMAIL]", code: 500 });
  });

  it("scrubs nested objects recursively", () => {
    const input = { meta: { email: "bob@test.org", count: 3 } };
    expect(scrubObject(input)).toEqual({
      meta: { email: "[EMAIL]", count: 3 },
    });
  });

  it("scrubs arrays of strings", () => {
    const input = { tags: ["alice@test.com", "safe-tag"] };
    expect(scrubObject(input)).toEqual({
      tags: ["[EMAIL]", "safe-tag"],
    });
  });

  it("passes through non-object values unchanged", () => {
    expect(scrubObject(42)).toBe(42);
    expect(scrubObject(null)).toBeNull();
    expect(scrubObject(undefined)).toBeUndefined();
  });
});
