<script lang="ts">
  import type { Mode } from "./api";
  import Icon from "./Icon.svelte";
  import Toggle from "./Toggle.svelte";
  import { t } from "./texts";

  let { mode = $bindable(), verify = $bindable() }: { mode: Mode; verify: boolean } = $props();
  let open = $state(false);
  const modes: Mode[] = ["auto", "iso", "dd"];
</script>

<section class="advanced" class:open>
  <button class="head" onclick={() => (open = !open)} aria-expanded={open}>
    <span>{t.advancedTitle}</span>
    <span class="chev"><Icon name="chevron" size={16} /></span>
  </button>
  {#if open}
    <div class="body fade-in">
      <div class="label">{t.modeLabel}</div>
      <div class="segmented" role="radiogroup" aria-label={t.modeLabel}>
        {#each modes as m (m)}
          <button role="radio" aria-checked={mode === m} class:active={mode === m} onclick={() => (mode = m)}>
            {t.modes[m]}
          </button>
        {/each}
      </div>
      <div class="hint">{t.modeHints[mode]}</div>
      <Toggle bind:checked={verify} label={t.verifyLabel} hint={t.verifyHint} />
    </div>
  {/if}
</section>

<style>
  .advanced {
    border-radius: var(--radius-card);
    border: 1px solid transparent;
    transition: background 0.2s var(--ease), border-color 0.2s var(--ease);
  }

  .advanced.open {
    background: var(--card);
    border-color: var(--card-border);
    box-shadow: var(--shadow-card);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 10px 16px;
    border: none;
    background: transparent;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-muted);
  }

  .chev {
    display: grid;
    transition: transform 0.2s var(--ease);
  }

  .open .chev {
    transform: rotate(180deg);
  }

  .body {
    padding: 0 16px 10px;
  }

  .label {
    font-size: 13px;
    margin-bottom: 6px;
  }

  .segmented {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    padding: 3px;
    gap: 3px;
    border-radius: var(--radius-control);
    background: var(--track);
  }

  .segmented button {
    height: 30px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-muted);
    font-weight: 600;
    transition: background 0.15s var(--ease), color 0.15s var(--ease);
  }

  .segmented button.active {
    background: var(--control-hover);
    color: var(--text);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.12);
  }

  .hint {
    margin: 6px 0 4px;
    font-size: 12.5px;
    color: var(--text-faint);
  }
</style>
