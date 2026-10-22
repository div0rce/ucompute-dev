// F-10-11 for the Dashboard (owner A): every block with realistic data (attention with a last problem, the computers,
// running and finished jobs, recent activity) has no axe violations.
import { act, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { refresh } from "../../state/live";
import { resetStores, scan, scenario } from "../../test/axe";
import { JOBS } from "../../test/mocks";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/dashboard (and its #/home alias) has no axe violations", async () => {
    const { fake, container } = await scenario("home");
    expect(await screen.findByRole("heading", { level: 1, name: "Dashboard" })).toBeTruthy();
    await screen.findByRole("grid", { name: "Computers" });
    fake.jobs = structuredClone(JOBS);
    await act(() => refresh("jobs"));
    for (const name of ["Computers", "Running now", "Recent activity"]) expect(await screen.findByRole("grid", { name })).toBeTruthy();
    expect(screen.getByText("Last problem")).toBeTruthy();
    await scan(container);
  }, 20000);
});
