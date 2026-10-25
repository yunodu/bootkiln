<script lang="ts">
  import type { ImageInfo } from "./api";
  import Card from "./Card.svelte";
  import Icon from "./Icon.svelte";
  import { bytes, fileName } from "./format";
  import { t } from "./texts";

  let {
    path,
    info,
    analyzing,
    error,
    dragOver,
    onBrowse,
  }: {
    path: string | null;
    info: ImageInfo | null;
    analyzing: boolean;
    error: string | null;
    dragOver: boolean;
    onBrowse: () => void;
  } = $props();

  let summary = $derived(info ? t.summary(info) : null);
</script>

<Card title={t.imageTitle}>
  {#if !path}
    <button class="drop" class:over={dragOver} onclick={onBrowse}>
      <span class="drop-icon"><Icon name="upload" size={22} /></span>
      <span>{t.imageDrop}</span>
      <span class="btn">{t.imageBrowse}</span>
    </button>
  {:else}
    <div class="file" class:over={dragOver}>
      <span class="file-icon" class:win={info?.windows}>
        <Icon name={info?.windows ? "windows" : "disc"} size={20} />
      </span>
      <span class="name">
        <span class="title" title={path}>{fileName(path)}</span>
        {#if analyzing}
          <span class="meta analyzing"><span class="spinner"></span>{t.imageAnalyzing}</span>
        {:else if summary && info}
          <span class="meta">{bytes(info.sizeBytes)} · {summary.title}</span>
        {/if}
      </span>
      <button class="btn ghost" onclick={onBrowse}>{t.imageChange}</button>
    </div>

    {#if !analyzing}
      <div class="details">
        {#if error}
          <div class="notice error"><Icon name="alert" size={16} /><span>{error}</span></div>
        {:else if info && summary}
          <div class="chip">{summary.detail}</div>
          {#if info.isoUnavailable}
            <div class="notice"><Icon name="info" size={16} /><span>{t.infoIsoUnavailable(info.isoUnavailable)}</span></div>
          {/if}
          {#if info.biosOnly}
            <div class="notice warn"><Icon name="alert" size={16} /><span>{t.warnBiosOnly}</span></div>
          {/if}
          {#if info.fileSystem === "ntfs"}
            <div class="notice"><Icon name="info" size={16} /><span>{t.warnNtfs(info.largeFiles)}</span></div>
          {/if}
          {#if info.patched.length > 0 && info.isoLabel && info.label}
            <div class="notice"><Icon name="info" size={16} /><span>{t.infoPatched(info.isoLabel, info.label)}</span></div>
          {/if}
        {/if}
      </div>
    {/if}
  {/if}
</Card>

<style>
  .drop {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 20px 16px;
    border-radius: 12px;
    background: transparent;
    border: 1.5px dashed var(--text-faint);
    transition: border-color 0.15s var(--ease), background 0.15s var(--ease);
  }

  .drop:hover,
  .drop.over {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .drop-icon {
    display: grid;
    place-items: center;
    width: 42px;
    height: 42px;
    border-radius: 12px;
    color: var(--accent);
    background: var(--accent-soft);
  }

  .file {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 6px 10px 12px;
    border-radius: var(--radius-control);
    border: 1px solid var(--control-border);
    background: var(--control);
    transition: border-color 0.15s var(--ease);
  }

  .file.over {
    border-color: var(--accent);
  }

  .file-icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 9px;
    color: var(--accent);
    background: var(--accent-soft);
  }

  .file-icon.win {
    color: #2b88d8;
    background: rgba(43, 136, 216, 0.14);
  }

  .name {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .title {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .analyzing {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .details {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 10px;
  }

  .details:empty {
    display: none;
  }

  .chip {
    align-self: flex-start;
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--accent);
    background: var(--accent-soft);
  }
</style>
