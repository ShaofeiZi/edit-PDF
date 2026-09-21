import React from "react";
import { Stack } from "@mantine/core";
import PreferencesSection from "@core/components/shared/config/configSections/preferences/PreferencesSection";
import { DefaultAppSettings } from "@app/components/shared/config/configSections/DefaultAppSettings";

interface GeneralSectionProps {
  /** Forwarded to the core section; the settings modal header already names it. */
  hideTitle?: boolean;
}

/**
 * Offline desktop preferences.
 *
 * The network updater and its controls are intentionally absent from this
 * build. Updates are performed by installing a separately verified package.
 */
const GeneralSection: React.FC<GeneralSectionProps> = () => (
  <Stack gap="lg">
    <PreferencesSection
      editorDefaultsSlot={<DefaultAppSettings />}
      hideUpdateSection
    />
  </Stack>
);

export default GeneralSection;
