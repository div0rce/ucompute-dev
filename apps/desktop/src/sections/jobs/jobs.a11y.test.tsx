// F-10-11 for Jobs (owner C): the section with E3 rows, its inspector tabs, the unsupported state and the
// #/performance alias.
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { installFakeDaemon, JOBS, NETWORK_ON, PEER } from "../../test/mocks";
import { resetStores, scan, scenario } from "../../test/axe";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/performance lands on Jobs with no axe violations", async () => {
    const { container } = await scenario("performance");
    expect(await screen.findByRole("heading", { level: 1, name: "Jobs" })).toBeTruthy();
    await screen.findByText(/Nothing is running/);
    await scan(container);
  }, 20000);

  it("jobs from E3 with a selected row and both inspector tabs", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] });
    await refreshStatus();
    window.location.hash = "#/jobs/chatcmpl-1a2b3c4d";
    const { container } = render(<App poll={false} />);
    const grid = await screen.findByRole("grid", { name: "Jobs" });
    await within(grid).findAllByText("Running");
    await scan(container);
    await user.click(screen.getByRole("tab", { name: "Timing & route" }));
    await screen.findByRole("img", { name: /^Phases:/ });
    await scan(container);
  }, 20000);

  it("a service without /api/jobs", async () => {
    installFakeDaemon({ jobs: "unsupported" });
    await refreshStatus();
    window.location.hash = "#/jobs";
    const { container } = render(<App poll={false} />);
    await screen.findByText("This service version does not report jobs");
    await scan(container);
  }, 20000);
});
