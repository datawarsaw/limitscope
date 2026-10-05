import { describe, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { AppErrorBoundary } from "./AppErrorBoundary";

function childTexts(node: ReactNode): string[] {
  if (node === null || node === undefined || typeof node === "boolean") return [];
  if (typeof node === "string" || typeof node === "number") return [String(node)];
  if (Array.isArray(node)) return node.flatMap(childTexts);
  const element = node as { props?: { children?: ReactNode } };
  return element.props ? childTexts(element.props.children) : [];
}

describe("AppErrorBoundary", () => {
  it("captures a render error into state", () => {
    const error = new Error("boom");
    expect(AppErrorBoundary.getDerivedStateFromError(error)).toEqual({ error });
  });

  it("renders its children when nothing failed", () => {
    const boundary = new AppErrorBoundary({ children: <main id="sentinel" /> });
    expect(boundary.render()).toMatchObject({ props: { id: "sentinel" } });
  });

  it("renders a recoverable, non-silent fallback naming the error", () => {
    const boundary = new AppErrorBoundary({ children: null });
    boundary.state = { error: new Error("Invalid time value") };
    const fallback = boundary.render();
    expect(fallback).toMatchObject({ props: { role: "alert" } });
    const text = childTexts(fallback).join(" ");
    expect(text).toContain("unexpected error");
    expect(text).toContain("Invalid time value");
  });

  it("logs the failure instead of swallowing it, and never throws", () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const boundary = new AppErrorBoundary({ children: null });
    expect(() =>
      boundary.componentDidCatch(new Error("boom"), { componentStack: "\n at App" }),
    ).not.toThrow();
    expect(errorSpy).toHaveBeenCalledWith(
      "LimitScope failed to render:",
      expect.objectContaining({ message: "boom" }),
      "\n at App",
    );
    errorSpy.mockRestore();
  });
});
