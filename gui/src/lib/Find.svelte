<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import DownloadProgress from "./DownloadProgress.svelte";
  import {
    findCheck,
    findOptions,
    findPreset,
    savedApiApp,
    type FindCheck,
    type FindCriteria,
    type FindOptions,
  } from "./api";
  import {
    cancel,
    downloadPicked,
    isActive,
    latestFindRun,
    loadMore,
    refreshOwned,
    search,
    store,
  } from "./store.svelte";

  let { onSettings, onImport }: { onSettings: () => void; onImport: () => void } = $props();

  /** The criteria typed as text. */
  type TextKey = {
    [K in keyof FindCriteria]: FindCriteria[K] extends string ? K : never;
  }[keyof FindCriteria];

  const f = store.find;
  let options = $state<FindOptions | null>(null);
  let loadError = $state<string | null>(null);
  let apiApp = $state<string | null>(null);
  let checked = $state<FindCheck | null>(null);
  let moreOpen = $state(false);

  onMount(async () => {
    try {
      [options, apiApp] = await Promise.all([findOptions(), savedApiApp()]);
      if (!f.criteria) f.criteria = $state.snapshot(options.defaults);
      // Setup may have pointed at another osu! since the results came in.
      if (f.rows.length > 0) void refreshOwned();
    } catch (e) {
      loadError = String(e);
    }
  });

  // Re-check the route and the fields whenever the form changes (local, instant).
  $effect(() => {
    const criteria = f.criteria ? $state.snapshot(f.criteria) : null;
    if (!criteria) return;
    let stale = false;
    findCheck(criteria).then((result) => {
      if (!stale) checked = result;
    });
    return () => (stale = true);
  });

  async function pickPreset(event: Event) {
    const preset = Number((event.currentTarget as HTMLSelectElement).value);
    // A preset resets the form, as in the terminal app; the limit stays.
    const limit = f.criteria!.limit;
    f.criteria = { ...(await findPreset(preset)), limit };
  }

  function toggleExtra(idx: number) {
    const extra = f.criteria!.extra;
    f.criteria!.extra = extra.includes(idx) ? extra.filter((e) => e !== idx) : [...extra, idx];
  }

  const needsApiApp = $derived(checked?.route === "osu" && !apiApp);
  const canSearch = $derived(!!checked?.route && !checked.problem && !needsApiApp && !f.searching);
  /** The shared "relevance" sort, which means newest ranked first on nzbasic. */
  const relevanceSort = $derived(options ? options.sorts.indexOf("relevance") : -1);
  const advancedSet = $derived.by(() => {
    const c = f.criteria;
    if (!c || !options) return 0;
    const d = options.defaults;
    const texts = [...textFields.map((t) => t.key), "limit" as const];
    const choices = [
      c.special !== d.special,
      c.explicit !== d.explicit,
      c.genre !== d.genre,
      c.language !== d.language,
      c.extra.length > 0,
    ];
    return texts.filter((k) => c[k].trim() !== "").length + choices.filter(Boolean).length;
  });

  const run = $derived(latestFindRun());
  const running = $derived(isActive(run));
  const pickedCount = $derived(f.picked.length);
  const pickedSize = $derived(
    f.rows.reduce((sum, r) => sum + (f.picked.includes(r.id) && r.size ? r.size : 0), 0),
  );

  let runCard = $state<HTMLElement | null>(null);
  /** The run just started from the download bar, until its card is shown. */
  let revealRun = $state<number | null>(null);

  async function startDownload() {
    const id = await downloadPicked();
    if (id !== null) revealRun = id;
  }

  function showRun() {
    runCard?.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  // The card renders below every result, so without this a download looks
  // like it did nothing. Waits for the run's first status to arrive.
  $effect(() => {
    if (revealRun === null || run?.id !== revealRun || !runCard) return;
    showRun();
    revealRun = null;
  });

  /** Live progress for the download bar, which stays on screen. */
  const runLine = $derived.by(() => {
    if (!run || !running) return null;
    if (run.phase === "preparing") return "Getting ready…";
    const settled = run.downloaded + run.skipped + run.failed;
    return `Downloading ${settled.toLocaleString()} of ${plural(run.total, "map set", "map sets")}`;
  });

  const inOsu = $derived(new Set(f.owned?.in_osu ?? []));
  const downloadedSets = $derived(new Set(f.owned?.downloaded ?? []));
  /** Where the user already has a set: in osu!, or downloaded but maybe not imported. */
  const have = (id: number): "osu" | "downloaded" | null =>
    inOsu.has(id) ? "osu" : downloadedSets.has(id) ? "downloaded" : null;
  const visibleRows = $derived(f.hideOwned ? f.rows.filter((r) => !have(r.id)) : f.rows);
  const hiddenCount = $derived(f.rows.length - visibleRows.length);
  const shownCount = $derived(visibleRows.length);

  // A hidden row can't be seen to be ticked, so it stops counting: hiding
  // (or a download that adds to what the user has) unticks it.
  $effect(() => {
    if (!f.hideOwned) return;
    const kept = f.picked.filter((id) => !have(id));
    if (kept.length !== f.picked.length) f.picked = kept;
  });

  // A finished download changes what's downloaded (and maybe what's in osu!).
  let refreshedFor = -1;
  $effect(() => {
    if (run?.phase === "done" && run.id !== refreshedFor) {
      refreshedFor = run.id;
      void refreshOwned();
    }
  });

  function togglePicked(id: number) {
    f.picked = f.picked.includes(id) ? f.picked.filter((p) => p !== id) : [...f.picked, id];
  }

  const plural = (n: number, one: string, many: string) =>
    `${n.toLocaleString()} ${n === 1 ? one : many}`;
  const stars = (span: [number, number] | null) =>
    !span ? null : span[0] === span[1] ? `${span[0].toFixed(2)}★` : `${span[0].toFixed(2)}–${span[1].toFixed(2)}★`;
  function size(bytes: number): string {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
    if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
    return `${Math.round(bytes / 1e3)} KB`;
  }

  const textFields: { key: TextKey; label: string; hint: string }[] = [
    { key: "stars", label: "Stars", hint: "7+  or  5.5..7" },
    { key: "ar", label: "AR", hint: "9+" },
    { key: "cs", label: "CS", hint: "4..5" },
    { key: "od", label: "OD", hint: "8+" },
    { key: "hp", label: "HP", hint: "6+" },
    { key: "bpm", label: "BPM", hint: "180+  or  170..190" },
    { key: "length", label: "Length (seconds)", hint: "90+  or  60..300" },
    { key: "keys", label: "Keys (mania)", hint: "4  or  4..7" },
    { key: "favourites", label: "Favourites", hint: "1000+" },
    { key: "ranked", label: "Ranked date", hint: "2020..2024" },
    { key: "artist", label: "Artist", hint: "" },
    { key: "creator", label: "Mapper", hint: "" },
    { key: "title", label: "Title", hint: "" },
  ];
</script>

<header class="page-head">
  <h1>Find maps</h1>
  <p class="muted">
    Search for maps without a collection: type a name, set filters, then download the ones you want.
  </p>
</header>

{#if loadError}
  <section class="card"><p class="error-text">{loadError}</p></section>
{:else if !options || !f.criteria}
  <p class="muted">Loading…</p>
{:else}
  {@const c = f.criteria}
  <form
    class="card"
    onsubmit={(e) => {
      e.preventDefault();
      if (canSearch) search();
    }}
  >
    <div class="add-row">
      <input
        class="text-input"
        bind:value={c.query}
        placeholder="Artist, title, mapper, tags…"
        aria-label="Search text"
      />
      <button class="primary" type="submit" disabled={!canSearch}>
        {f.searching ? "Searching…" : "Find"}
      </button>
    </div>

    <div class="selects">
      <label class="field">
        <span class="field-label">Preset</span>
        <select value={c.preset} onchange={pickPreset}>
          {#each options.presets as label, i (label)}<option value={i}>{label}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="field-label">Mode</span>
        <select bind:value={c.mode}>
          {#each options.modes as label, i (label)}<option value={i}>{label}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="field-label">Status</span>
        <select bind:value={c.status}>
          {#each options.statuses as label, i (label)}<option value={i}>{label}</option>{/each}
        </select>
      </label>
      <label class="field">
        <span class="field-label">Sort</span>
        <select bind:value={c.sort}>
          {#each options.sorts as label, i (label)}<option value={i}>{label}</option>{/each}
        </select>
      </label>
    </div>

    {#if checked}
      {#if checked.problem}
        <p class="error-text small">{checked.problem}</p>
      {:else if needsApiApp}
        <p class="warn-text small">
          This search goes to osu!, which needs your own osu! API app.
          <button type="button" class="link" onclick={onSettings}>Add it in Settings</button>
          or use a nzbasic-only filter (Special, or sort by BPM or length).
        </p>
      {:else if checked.route === "osu"}
        <p class="muted small">Searches osu! with your API app.</p>
      {:else}
        <p class="muted small">
          Searches nzbasic's map database (no account needed).
          {#if c.sort === relevanceSort}Relevance there means newest ranked first.{/if}
        </p>
      {/if}
    {/if}
    {#if f.error}
      <p class="error-text small">{f.error}</p>
    {/if}
    <button type="button" class="link more" onclick={() => (moreOpen = !moreOpen)}>
      {moreOpen ? "Fewer filters" : "More filters"}{#if !moreOpen && advancedSet > 0}&nbsp;({advancedSet} set){/if}
    </button>

    {#if moreOpen}
      <div class="grid">
        {#each textFields as field (field.key)}
          <label class="field">
            <span class="field-label">{field.label}</span>
            <input class="text-input" bind:value={c[field.key]} placeholder={field.hint} />
          </label>
        {/each}
        <label class="field">
          <span class="field-label">Special (nzbasic)</span>
          <select bind:value={c.special}>
            {#each options.specials as label, i (label)}<option value={i}>{label}</option>{/each}
          </select>
        </label>
        <label class="field">
          <span class="field-label">Explicit</span>
          <select bind:value={c.explicit}>
            {#each options.explicit as label, i (label)}<option value={i}>{label}</option>{/each}
          </select>
        </label>
        <label class="field">
          <span class="field-label">Genre</span>
          <select bind:value={c.genre}>
            {#each options.genres as label, i (label)}<option value={i}>{label}</option>{/each}
          </select>
        </label>
        <label class="field">
          <span class="field-label">Language</span>
          <select bind:value={c.language}>
            {#each options.languages as label, i (label)}<option value={i}>{label}</option>{/each}
          </select>
        </label>
        <label class="field">
          <span class="field-label">Max results (nzbasic)</span>
          <input class="text-input" bind:value={c.limit} placeholder="500" />
        </label>
        <div class="field">
          <span class="field-label">Has</span>
          <div class="checks">
            {#each options.extras as label, i (label)}
              <label class="check">
                <input type="checkbox" checked={c.extra.includes(i)} onchange={() => toggleExtra(i)} />
                {label}
              </label>
            {/each}
          </div>
        </div>
      </div>
      <p class="muted small">
        Numbers: <code>7</code>, <code>7+</code>, <code>&lt;7</code> or a range <code>5.5..7</code>.
        Artist, mapper and title match the whole phrase on osu!, any part on nzbasic.
      </p>
    {/if}

  </form>

  {#if f.searched && !f.searching}
    {#if f.rows.length === 0}
      <section class="card"><p class="muted">No maps match. Try fewer filters.</p></section>
    {:else}
      <section class="card results">
        <div class="results-head">
          <h2>
            {shownCount < f.total
              ? `Showing ${shownCount.toLocaleString()} of ${plural(f.total, "result", "results")}`
              : plural(shownCount, "result", "results")}
          </h2>
          <div class="actions">
            <label class="check small">
              <input type="checkbox" bind:checked={f.hideOwned} />
              Hide maps I already have{#if f.hideOwned && hiddenCount > 0}&nbsp;({hiddenCount.toLocaleString()} hidden){/if}
            </label>
            <button class="secondary" onclick={() => (f.picked = visibleRows.map((r) => r.id))}>Tick all</button>
            <button class="secondary" onclick={() => (f.picked = [])}>Untick all</button>
          </div>
        </div>
        <ul class="rows">
          {#each visibleRows as row (row.id)}
            {@const owned = have(row.id)}
            <li class:have-osu={owned === "osu"} class:have-downloaded={owned === "downloaded"}>
              <label class="row">
                <input type="checkbox" checked={f.picked.includes(row.id)} onchange={() => togglePicked(row.id)} />
                <img
                  class="cover"
                  src="https://assets.ppy.sh/beatmaps/{row.id}/covers/list.jpg"
                  alt=""
                  loading="lazy"
                  onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = "hidden")}
                />
                <span class="row-main">
                  <span class="row-title">
                    {row.title ? `${row.artist} – ${row.title}` : `Map set ${row.id}`}
                    {#if owned === "osu"}
                      <span class="tag tag-osu">In osu!</span>
                    {:else if owned === "downloaded"}
                      <span class="tag tag-downloaded" title="In your download folder; import it to play it">Downloaded</span>
                    {/if}
                  </span>
                  <span class="muted small">
                    {#if row.creator}mapped by {row.creator}{/if}
                    {#if row.status}· {row.status}{/if}
                    {#if stars(row.stars)}· {stars(row.stars)}{/if}
                    {#if row.difficulties > 0}
                      · {plural(row.difficulties, "difficulty", "difficulties")}{f.route === "nzbasic" ? " matching" : ""}
                    {/if}
                    {#if row.favourites !== null}· ♥ {row.favourites.toLocaleString()}{/if}
                    {#if row.size}· {size(row.size)}{/if}
                  </span>
                </span>
              </label>
              <button class="link" onclick={() => openUrl(`https://osu.ppy.sh/beatmapsets/${row.id}`)}>
                osu! page
              </button>
            </li>
          {/each}
        </ul>
        {#if visibleRows.length === 0}
          <p class="muted small">You already have every map shown here.</p>
        {/if}
        {#if f.owned?.library_error}
          <p class="muted small">
            Couldn't read your osu! library, so only downloaded maps are marked: {f.owned.library_error}
          </p>
        {/if}
        {#if f.nextCursor}
          <div class="actions left">
            <button class="secondary" onclick={loadMore} disabled={f.loadingMore}>
              {f.loadingMore ? "Loading…" : "Load more"}
            </button>
          </div>
        {/if}
      </section>

      <div class="actions download-bar">
        {#if f.downloadError}
          <p class="error-text small">{f.downloadError}</p>
        {:else if runLine}
          <p class="muted small">
            {runLine} · <button class="link" onclick={showRun}>Show progress</button>
          </p>
        {:else if pickedCount === 0}
          <p class="muted small">Tick the map sets you want.</p>
        {/if}
        <button class="primary" onclick={startDownload} disabled={pickedCount === 0}>
          Download {plural(pickedCount, "map set", "map sets")}{#if pickedSize > 0}&nbsp;(about {size(pickedSize)}){/if}
        </button>
      </div>
    {/if}
  {/if}

  {#if run}
    <section class="card" bind:this={runCard}>
      <div class="entry">
        <div class="entry-main">
          <span class="entry-title">Search “{run.name}”</span>
        </div>
        {#if running}
          <div class="entry-buttons">
            <button class="secondary" onclick={() => cancel(run)}>Cancel</button>
          </div>
        {/if}
      </div>
      <DownloadProgress status={run} />
      {#if run.phase === "done"}
        <div class="actions left">
          <button class="primary" onclick={onImport}>Continue to import</button>
        </div>
      {/if}
    </section>
  {/if}
{/if}

<style>
  .selects {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 10px;
    margin-top: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: 10px;
    margin-top: 10px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  select {
    font: inherit;
    padding: 7px 8px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
  }
  .more {
    margin-top: 4px;
  }
  .checks {
    display: flex;
    gap: 12px;
    padding-top: 6px;
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .results-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .results-head h2 {
    margin: 0;
  }
  .results-head .actions {
    margin-top: 0;
  }
  .rows {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .rows li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 0;
    border-top: 1px solid var(--border);
  }
  .row {
    flex: 1;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    cursor: pointer;
    min-width: 0;
  }
  .row input {
    margin-top: 4px;
  }
  .cover {
    width: 44px;
    height: 44px;
    flex: none;
    border-radius: 6px;
    object-fit: cover;
    background: var(--border);
  }
  .download-bar {
    position: sticky;
    bottom: 0;
    padding: 12px 0;
    background: var(--bg);
  }
  .row-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .row-title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  /* Maps the user already has: a coloured edge and a tag, like osu!'s own listing. */
  .rows li.have-osu,
  .rows li.have-downloaded {
    padding-left: 10px;
    box-shadow: inset 3px 0 0 var(--have);
  }
  .rows li.have-osu {
    --have: var(--ok);
  }
  .rows li.have-downloaded {
    --have: var(--warn);
  }
  .tag {
    display: inline-block;
    margin-left: 6px;
    padding: 0 7px;
    border-radius: 999px;
    font-size: 0.75rem;
    font-weight: 600;
    vertical-align: 1px;
    border: 1px solid currentColor;
  }
  .tag-osu {
    color: var(--ok);
  }
  .tag-downloaded {
    color: var(--warn);
  }
  .results-head .check {
    color: var(--muted);
  }
</style>
