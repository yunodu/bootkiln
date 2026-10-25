<script lang="ts" module>
  import type { Release } from "./api";

  export type Resolved = { state: "loading" } | { state: "ok"; release: Release } | { state: "error"; error: string };
</script>

<script lang="ts">
  import type { CatalogEntry } from "./api";
  import Icon from "./Icon.svelte";
  import { bytes } from "./format";
  import { t } from "./texts";

  let {
    entries,
    resolved,
    language = $bindable(),
    onDownload,
    onClose,
  }: {
    entries: CatalogEntry[];
    resolved: Record<string, Resolved>;
    language: string;
    onDownload: (release: Release, entry: CatalogEntry) => void;
    onClose: () => void;
  } = $props();

  let selectedId = $state<string | null>(null);
  let selected = $derived(entries.find((e) => e.id === selectedId) ?? null);
  let selectedState = $derived(selected ? resolved[selected.id] : undefined);
  let release = $derived(selectedState?.state === "ok" ? selectedState.release : null);
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<div class="backdrop" role="presentation" onclick={onClose}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby="catalog-title">
  <header>
    <div>
      <h2 id="catalog-title">{t.catalogTitle}</h2>
      <p>{t.catalogSubtitle}</p>
    </div>
    <button class="btn ghost close" onclick={onClose} aria-label={t.catalogClose}><Icon name="close" size={16} /></button>
  </header>

  <div class="grid" role="radiogroup" aria-label={t.catalogTitle}>
    {#each entries as entry (entry.id)}
      {@const state = resolved[entry.id] ?? { state: "loading" }}
      <button
        class="tile"
        class:active={selectedId === entry.id}
        class:unavailable={state.state === "error"}
        role="radio"
        aria-checked={selectedId === entry.id}
        disabled={state.state === "error"}
        title={state.state === "error" ? state.error : undefined}
        onclick={() => (selectedId = entry.id)}
      >
        <span class="mono" style:background={entry.color}>{entry.name[0]}</span>
        <span class="text">
          <span class="name">{entry.name}</span>
          <span class="summary">{entry.summary}</span>
          <span class="version">
            {#if state.state === "loading"}
              <span class="spinner small"></span>{t.catalogLoading}
            {:else if state.state === "error"}
              {t.catalogUnavailable}
            {:else}
              {state.release.version}{#if state.release.size}&nbsp;· {bytes(state.release.size)}{/if}
            {/if}
          </span>
        </span>
      </button>
    {/each}
  </div>

  <footer>
    {#if selected?.windows && release}
      <label class="language">
        <span>{t.catalogLanguage}</span>
        <select bind:value={language}>
          {#each release.languages as name (name)}<option value={name}>{name}</option>{/each}
        </select>
      </label>
    {/if}
    <button
      class="btn primary large"
      disabled={!release || !selected}
      onclick={() => release && selected && onDownload(release, selected)}
    >
      {#if selected && release}
        {t.catalogDownload(selected.name, release.version, release.size ? bytes(release.size) : "")}
      {:else if selected && selectedState?.state === "loading"}
        <span class="spinner light"></span>{t.catalogLoading}
      {:else}
        {t.catalogPick}
      {/if}
    </button>
  </footer>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--backdrop);
    backdrop-filter: blur(3px);
    animation: appear 0.2s var(--ease);
  }

  .sheet {
    position: fixed;
    inset: 28px 14px 14px;
    display: flex;
    flex-direction: column;
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid var(--card-border);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    animation: rise 0.25s var(--ease);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 18px 18px 10px;
  }

  h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
  }

  header p {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .close {
    width: 32px;
    height: 32px;
    padding: 0;
  }

  .grid {
    flex: 1;
    overflow-y: auto;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 8px;
    padding: 6px 18px 12px;
    align-content: start;
  }

  .tile {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px;
    text-align: left;
    border-radius: 12px;
    border: 1px solid var(--control-border);
    background: var(--card);
    transition: border-color 0.15s var(--ease), box-shadow 0.15s var(--ease), background 0.15s var(--ease);
  }

  .tile:hover:not(:disabled) {
    background: var(--control-hover);
  }

  .tile.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }

  .tile.unavailable {
    opacity: 0.55;
  }

  .mono {
    flex: none;
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 11px;
    color: #fff;
    font-family: var(--font-display);
    font-size: 19px;
    font-weight: 700;
    box-shadow: inset 0 -2px 0 rgba(0, 0, 0, 0.15);
  }

  .text {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .summary,
  .version {
    font-size: 12px;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .version {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
  }

  .spinner.small {
    width: 11px;
    height: 11px;
    border-width: 1.5px;
  }

  .spinner.light {
    border-color: rgba(255, 255, 255, 0.35);
    border-top-color: #fff;
  }

  footer {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 18px 16px;
    border-top: 1px solid var(--card-border);
  }

  footer .btn {
    width: 100%;
  }

  .language {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-size: 13px;
  }

  select {
    flex: 1;
    max-width: 260px;
    height: 34px;
    padding: 0 10px;
    border-radius: var(--radius-control);
    border: 1px solid var(--control-border);
    background: var(--control);
    font: inherit;
    color: inherit;
  }

  @keyframes appear {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(16px);
    }
  }
</style>
