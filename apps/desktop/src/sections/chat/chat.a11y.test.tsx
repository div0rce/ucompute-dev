// F-10-11 for Chat (owner D): the section with a conversation, the Run panel, a peer destination and the
// New conversation dialog open.
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetStores, scan, scenario } from "../../test/axe";
import { PEER } from "../../test/mocks";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/chat has no axe violations", async () => {
    const { container } = await scenario("chat");
    expect(await screen.findByRole("heading", { level: 1, name: "Chat" })).toBeTruthy();
    await screen.findByRole("textbox", { name: "Message" });
    expect(screen.getByRole("complementary", { name: "Run details" })).toBeTruthy();
    await scan(container);
  }, 20000);

  it("#/chat with a paired device as the destination and the New conversation dialog open", async () => {
    const user = userEvent.setup();
    const { container } = await scenario("chat");
    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(screen.getByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    await screen.findByText(/Prompts run on Studio Mac/);
    await scan(container);
    await user.click(screen.getByRole("button", { name: "New conversation" }));
    expect(await screen.findByRole("dialog", { name: "Start a new conversation?" })).toBeTruthy();
    await scan(container);
  }, 20000);
});
