<script lang="ts">
  import type { Drive } from "./api";
  import Card from "./Card.svelte";
  import Icon from "./Icon.svelte";
  import { bytes } from "./format";
  import { t } from "./texts";

  let { drives, selected = $bindable() }: { drives: Drive[]; selected: number | null } = $props();
</script>

<Card title={t.driveTitle}>
  {#if drives.length === 0}
    <div class="empty">
      <div class="pulse"><Icon name="usb" size={22} /></div>
      <div>
        <div>{t.driveEmpty}</div>
        <div class="faint">{t.driveEmptyHint}</div>
      </div>
    </div>
  {:else}
    <div class="list" role="radiogroup" aria-label={t.driveTitle}>
      {#each drives as drive (drive.number)}
        <button
          class="drive"
          class:active={selected === drive.number}
          role="radio"
          aria-checked={selected === drive.number}
          onclick={() => (selected = drive.number)}
        >
          <span class="icon"><Icon name={drive.bus === "SD" ? "sd" : "usb"} size={20} /></span>
          <span class="name">
            <span class="model">{drive.model || "Removable drive"}</span>
            <span class="meta">
              {drive.bus}{#if drive.mountPoints.length > 0}&nbsp;· {drive.mountPoints.join(" ")}{/if}
            </span>
          </span>
          <span class="size">{bytes(drive.sizeBytes)}</span>
        </button>
      {/each}
    </div>
  {/if}
</Card>

<style>
  .empty {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 6px 2px;
  }

  .faint {
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .pulse {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 12px;
    color: var(--accent);
    background: var(--accent-soft);
    animation: pulse 2.2s ease-in-out infinite;
  }

  @keyframes pulse {
    50% {
      box-shadow: 0 0 0 6px var(--accent-soft);
    }
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .drive {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 10px 12px;
    text-align: left;
    border-radius: var(--radius-control);
    border: 1px solid var(--control-border);
    background: var(--control);
    transition: border-color 0.15s var(--ease), background 0.15s var(--ease), box-shadow 0.15s var(--ease);
  }

  .drive:hover {
    background: var(--control-hover);
  }

  .drive.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }

  .icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 9px;
    background: var(--info-bg);
    color: var(--text-muted);
  }

  .active .icon {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .name {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .model {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .size {
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
  }
</style>
