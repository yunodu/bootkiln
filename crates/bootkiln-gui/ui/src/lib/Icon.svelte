<script lang="ts">
  type Name =
    | "usb"
    | "sd"
    | "disc"
    | "windows"
    | "check"
    | "alert"
    | "info"
    | "chevron"
    | "eject"
    | "upload"
    | "close";

  let { name, size = 18 }: { name: Name; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.8"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#if name === "usb"}
    <rect x="6" y="9" width="12" height="13" rx="2.5" />
    <path d="M8.5 9V3.5h7V9" />
    <path d="M10.5 6h0.01M13.5 6h0.01" stroke-width="2.4" />
  {:else if name === "sd"}
    <path d="M8 2.5h8.5L20 6v13.5A2 2 0 0 1 18 21.5H8A2 2 0 0 1 6 19.5v-15a2 2 0 0 1 2-2Z" />
    <path d="M10 6v3M13 6v3M16 6v3" />
  {:else if name === "disc"}
    <circle cx="12" cy="12" r="9" />
    <circle cx="12" cy="12" r="2.5" />
  {:else if name === "windows"}
    <path d="M3.5 5.5 10.5 4.5v7h-7zM12.5 4.2l8-1.2v8.5h-8zM3.5 12.5h7v7l-7-1zM12.5 12.5h8V21l-8-1.2z" fill="currentColor" stroke="none" />
  {:else if name === "check"}
    <path d="M5 12.5 10 17.5 19.5 7" />
  {:else if name === "alert"}
    <path d="M12 3.5 21.5 20h-19z" />
    <path d="M12 10v4.5M12 17.5h0.01" />
  {:else if name === "info"}
    <circle cx="12" cy="12" r="9" />
    <path d="M12 11v5.5M12 7.5h0.01" />
  {:else if name === "chevron"}
    <path d="m8 10 4 4 4-4" />
  {:else if name === "eject"}
    <path d="M12 5 19.5 14h-15z" />
    <path d="M5 18.5h14" />
  {:else if name === "upload"}
    <path d="M12 15.5V4M7 8.5 12 4l5 4.5" />
    <path d="M4.5 15v3.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15" />
  {:else if name === "close"}
    <path d="M6 6l12 12M18 6 6 18" />
  {/if}
</svg>
