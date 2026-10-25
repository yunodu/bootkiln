<script lang="ts" module>
  import type { DownloadStage } from "./api";

  /** A catalog download shown in place of an image. */
  export interface DownloadView {
    name: string;
    version: string;
    color: string;
    status: "running" | "paused" | "failed";
    stage: DownloadStage;
    done: number;
    total: number;
    error: string | null;
  }
</script>

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
    download,
    onBrowse,
    onCatalog,
    onPause,
    onResume,
    onCancelDownload,
  }: {
    path: string | null;
    info: ImageInfo | null;
    analyzing: boolean;
    error: string | null;
    dragOver: boolean;
    download: DownloadView | null;
    onBrowse: () => void;
    onCatalog: () => void;
    onPause: () => void;
    onResume: () => void;
    onCancelDownload: () => void;
  } = $props();

  let summary = $derived(info ? t.summary(info) : null);
  let fraction = $derived(download && download.total > 0 ? download.done / download.total : 0);
</script>

<Card title={t.imageTitle}>
  {#if download}
    <div class="file">
      <span class="mono" style:background={download.color}>{download.name[0]}</span>
      <span class="name">
        <span class="title">{download.name} {download.version}</span>
        <span class="meta">
          {#if download.status === "paused"}
            {t.downloadPaused} · {t.downloadOf(bytes(download.done), bytes(download.total))}
          {:else if download.status === "failed"}
            {t.downloadFailed}
          {:else if download.stage === "checking"}
            {t.downloadChecking}
          {:else}
            {t.downloadOf(bytes(download.done), bytes(download.total))} · {Math.floor(fraction * 100)}%
          {/if}
        </span>
      </span>
      {#if download.status === "running"}
        <button class="btn ghost" onclick={onPause}>{t.downloadPause}</button>
      {:else}
        <button class="btn ghost" onclick={onResume}>
          {download.status === "failed" ? t.downloadRetry : t.downloadResume}
        </button>
      {/if}
      <button class="btn ghost icon" onclick={onCancelDownload} aria-label={t.downloadCancel} title={t.downloadCancel}>
        <Icon name="close" size={16} />
      </button>
    </div>
    <div class="bar" class:paused={download.status !== "running"}>
      <div class="fill" style:width={`${fraction * 100}%`}></div>
    </div>
    {#if download.error}
      <div class="details">
        <div class="notice error"><Icon name="alert" size={16} /><span>{download.error}</span></div>
      </div>
    {/if}
  {:else if !path}
    <div class="drop" class:over={dragOver}>
      <span class="drop-icon"><Icon name="upload" size={22} /></span>
      <span>{t.imageDrop}</span>
      <span class="choices">
        <button class="btn" onclick={onBrowse}>{t.imageBrowse}</button>
        <span class="or">{t.imageOr}</span>
        <button class="btn" onclick={onCatalog}>{t.imageDownload}</button>
      </span>
    </div>
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
      <button class="btn ghost" onclick={onCatalog}>{t.imageDownloadShort}</button>
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
    color: var(--text-muted);
    border: 1.5px dashed var(--text-faint);
    transition: border-color 0.15s var(--ease), background 0.15s var(--ease);
  }

  .drop.over {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .choices {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .or {
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .mono {
    flex: none;
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 9px;
    color: #fff;
    font-family: var(--font-display);
    font-size: 16px;
    font-weight: 700;
  }

  .icon {
    width: 36px;
    padding: 0;
  }

  .bar {
    height: 6px;
    margin-top: 10px;
    border-radius: 3px;
    background: var(--track);
    overflow: hidden;
  }

  .bar .fill {
    height: 100%;
    border-radius: 3px;
    background: var(--accent-gradient);
    transition: width 0.25s linear;
  }

  .bar.paused .fill {
    background: var(--text-faint);
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
