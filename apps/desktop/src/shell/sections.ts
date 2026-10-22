// Route -> section component and rail entry. Section contract: `export default function <Section>(): JSX.Element`.
import type { JSX } from "react";
import type { Section } from "../router";
import Activity from "../sections/activity/Activity";
import Chat from "../sections/chat/Chat";
import Dashboard from "../sections/dashboard/Dashboard";
import Devices from "../sections/devices/Devices";
import Jobs from "../sections/jobs/Jobs";
import Models from "../sections/models/Models";
import Settings from "../sections/settings/Settings";
import type { IconName } from "../ui/icons";

export const SECTION_META: Record<Section, { label: string; icon: IconName }> = {
  dashboard: { label: "Dashboard", icon: "layout-dashboard" },
  devices: { label: "Devices", icon: "monitor" },
  models: { label: "Models", icon: "box" },
  jobs: { label: "Jobs", icon: "list-checks" },
  chat: { label: "Chat", icon: "message-square" },
  activity: { label: "Activity", icon: "activity" },
  settings: { label: "Settings", icon: "settings" },
};

export const SECTION_COMPONENTS: Record<Section, () => JSX.Element> = { dashboard: Dashboard, devices: Devices, models: Models, jobs: Jobs, chat: Chat, activity: Activity, settings: Settings };
