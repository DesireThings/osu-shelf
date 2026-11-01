<script lang="ts">
  import { skipWaiting, type DownloadStatus, type MirrorForecast } from "./api";

  let { status }: { status: DownloadStatus } = $props();

  let showFailures = $state(false);
  let skipError = $state<string | null>(null);

  async function skip() {
    skipError = null;
    try {
      await skipWaiting(status.id);
    } catch (e) {
      skipError = String(e);
    }
  }

  const settled = $derived(status.downloaded + status.skipped + status.failed);
  const percent = $derived(status.total > 0 ? Math.min(100, (settled / status.total) * 100) : 0);

  function size(bytes: number): string {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
    if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
    return `${Math.round(bytes / 1e3)} KB`;
  }

  // The engine says how long until a waiting set retries as of each update;
  // count down from there so the number moves between updates.
  const receivedAt = $derived.by(() => {
    void status;
    return Date.now();
  });
  let now = $state(Date.now());
  $effect(() => {
    if (status.waiting === 0) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  const retryIn = $derived(
    status.retry_in_secs === null
      ? null
      : Math.max(0, status.retry_in_secs - Math.max(0, Math.floor((now - receivedAt) / 1000))),
  );

  function wait(secs: number): string {
    return secs < 90 ? `in ${secs} s` : `in about ${Math.ceil(secs / 60)} min`;
  }

  /** `45 s`, `2 min`, `1 h 12 min` — for mirror waits in the ETA's tooltip. */
  function shortDuration(secs: number): string {
    if (secs < 60) return `${secs} s`;
    if (secs < 3600) return `${Math.round(secs / 60)} min`;
    if (secs < 86400) {
      const hours = Math.floor(secs / 3600);
      const minutes = Math.floor((secs % 3600) / 60);
      return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
    }
    const days = Math.floor(secs / 86400);
    const hours = Math.floor((secs % 86400) / 3600);
    return hours > 0 ? `${days} d ${hours} h` : `${days} d`;
  }

  // The forecast is a lower bound: it covers when the mirrors can send every
  // outstanding request, not how long those requests then take to arrive.
  const eta = $derived.by(() => {
    const secs = status.eta_secs;
    if (secs === null || secs === 0) return null;
    if (secs < 60) return "under a minute left";
    if (secs >= 86400) return "over a day left";
    return `about ${shortDuration(secs)} left`;
  });

  /** Live mirror cooldowns and budget blocks, listed in the ETA's tooltip. */
  const mirrorHolds = $derived(
    status.mirrors
      .map((m: MirrorForecast): string | null => {
        const blocked = m.cooldown_secs ?? m.budget_secs;
        if (blocked === null) return null;
        const reason = m.cooldown_secs !== null ? "cooling down" : "budget blocked";
        const left =
          m.remaining_requests !== null ? `, ${m.remaining_requests} requests left` : "";
        return `${m.mirror}: ${reason} for ${shortDuration(blocked)}${left}`;
      })
      .filter((hold): hold is string => hold !== null)
      .join("\n"),
  );

  const summary = $derived.by(() => {
    const parts = [`${status.downloaded} new`];
    const alreadyHad = status.skipped - status.skipped_busy;
    if (alreadyHad > 0) parts.push(`${alreadyHad} you already had`);
    if (status.skipped_busy > 0) parts.push(`${status.skipped_busy} skipped for now`);
    if (status.failed > 0) parts.push(`${status.failed} couldn't be downloaded`);
    return parts.join(" · ");
  });
</script>

<div class="progress" data-phase={status.phase}>
  {#if status.phase === "queued"}
    <p class="muted small">Waiting for the download above to finish…</p>
  {:else if status.phase === "preparing"}
    <div class="bar indeterminate"><span></span></div>
    <p class="muted small">Getting ready: checking which maps you already have…</p>
  {:else if status.phase === "downloading"}
    <div class="bar"><span style:width="{percent}%"></span></div>
    <p class="small">
      {settled} of {status.total} map sets
      {#if status.active > 0}· {status.active} downloading now{/if}
      {#if status.waiting > 0}· {status.waiting} waiting for a free mirror{/if}
      {#if status.bytes_done > 0}· {size(status.bytes_done)}{/if}
      {#if eta}<span title={mirrorHolds || undefined}>· {eta}</span>{/if}
    </p>
    {#if status.waiting > 0}
      <div class="waiting">
        <p class="muted small">
          The mirrors that have {status.waiting === 1 ? "this map set" : "these map sets"} are busy
          right now, so {status.waiting === 1 ? "it's" : "they're"} tried again
          {retryIn ? wait(retryIn) : "in a moment"}. You can wait, or skip
          {status.waiting === 1 ? "it" : "them"} and get {status.waiting === 1 ? "it" : "them"} later.
        </p>
        <button class="secondary" onclick={skip}>Skip {status.waiting === 1 ? "it" : "these"} for now</button>
      </div>
      {#if skipError}
        <p class="error-text small">{skipError}</p>
      {/if}
    {/if}
  {:else if status.phase === "done"}
    <div class="bar done"><span style:width="100%"></span></div>
    <p class="small {status.failed > 0 || status.skipped_busy > 0 ? 'warn-text' : 'ok'}">
      Done: {summary}
    </p>
    {#if status.skipped_busy > 0}
      <p class="muted small">
        To get the {status.skipped_busy === 1 ? "map set" : "map sets"} skipped for now, press Check
        again later. Only what's missing is downloaded.
      </p>
    {/if}
  {:else if status.phase === "cancelled"}
    <p class="muted small">Cancelled. {status.downloaded} new map sets were downloaded before stopping.</p>
  {:else}
    <p class="error-text small">The download failed: {status.error ?? "unknown error"}</p>
  {/if}

  {#if status.warning}
    <p class="warn-text small">{status.warning}</p>
  {/if}

  {#if status.failures.length > 0}
    <button class="link" onclick={() => (showFailures = !showFailures)}>
      {showFailures ? "Hide" : "Show"} the {status.failures.length} that couldn't be downloaded
    </button>
    {#if showFailures}
      <ul class="failures small">
        {#each status.failures as failure (failure.beatmapset_id)}
          <li>
            {failure.title ?? `Map set ${failure.beatmapset_id}`}
            <span class="muted">({failure.reason})</span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>
