<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, subtitle, children, aside }: { title: string; subtitle?: string; children: Snippet; aside?: Snippet } =
    $props();
</script>

<section class="card fade-in">
  <header>
    <div>
      <h2>{title}</h2>
      {#if subtitle}<p>{subtitle}</p>{/if}
    </div>
    {#if aside}{@render aside()}{/if}
  </header>
  {@render children()}
</section>

<style>
  .card {
    background: var(--card);
    border: 1px solid var(--card-border);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-card);
    padding: 14px 16px 16px;
    backdrop-filter: blur(18px);
  }

  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 10px;
  }

  h2 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-muted);
  }

  p {
    margin: 2px 0 0;
    font-size: 12.5px;
    color: var(--text-faint);
  }
</style>
