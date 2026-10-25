<script lang="ts">
  import type { WindowsInfo, WindowsOptions } from "./api";
  import Card from "./Card.svelte";
  import Toggle from "./Toggle.svelte";
  import { t } from "./texts";

  let {
    windows,
    options = $bindable(),
    localAccount = $bindable(),
    accountName = $bindable(),
  }: { windows: WindowsInfo; options: WindowsOptions; localAccount: boolean; accountName: string } = $props();

  let isWindows10 = $derived(windows.isWindows11 === false);
  $effect(() => {
    if (isWindows10) options.skipHardwareChecks = false;
  });
</script>

<Card title={t.windowsTitle} subtitle={t.windowsSubtitle}>
  <Toggle
    bind:checked={options.skipHardwareChecks}
    label={t.winSkipHw}
    hint={isWindows10 ? t.winSkipHwWin10 : t.winSkipHwHint}
    disabled={isWindows10}
  />
  <Toggle bind:checked={options.skipOnlineAccount} label={t.winSkipOnline} hint={t.winSkipOnlineHint} />
  <Toggle bind:checked={localAccount} label={t.winLocal} hint={t.winLocalHint}>
    <input class="field" bind:value={accountName} placeholder={t.winLocalPlaceholder} maxlength="20" spellcheck="false" />
  </Toggle>
  <Toggle bind:checked={options.copyRegion} label={t.winRegion} hint={t.winRegionHint} />
  <Toggle bind:checked={options.skipPrivacyQuestions} label={t.winPrivacy} hint={t.winPrivacyHint} />
  <Toggle bind:checked={options.disableBitlocker} label={t.winBitlocker} hint={t.winBitlockerHint} />
</Card>

<style>
  .field {
    width: 100%;
    height: 36px;
    padding: 0 12px;
    border-radius: var(--radius-control);
    border: 1px solid var(--control-border);
    background: var(--control);
    user-select: text;
    outline: none;
    transition: border-color 0.15s var(--ease), box-shadow 0.15s var(--ease);
  }

  .field:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
</style>
