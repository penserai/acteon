# Acteon Admin UI -- Component Library Specification

> Every component an engineer needs to build the Acteon Admin UI.
> Each component includes anatomy, variants, states, accessibility, and Tailwind CSS guidance.

---

## Table of Contents

1. [Navigation](#navigation)
2. [Buttons](#buttons)
3. [Inputs](#inputs)
4. [Data Display](#data-display)
5. [Tables](#tables)
6. [Feedback](#feedback)
7. [Layout](#layout)
8. [Specialized](#specialized)

---

## Navigation

### Sidebar

**Anatomy**: Vertical navigation rail on the left side of the viewport.

```
+----------------------------------+
| [Logo]  Acteon      [Collapse]   |
|----------------------------------|
| [icon] Dashboard                 |
| [icon] Dispatch                  |
| [icon] Rules                     |
| [icon] Audit Trail               |
| [icon] Events                    |
| [icon] Groups                    |
| [icon] Chains                    |
| [icon] Approvals          [2]   |
| [icon] Circuit Breakers          |
| [icon] Dead-Letter Queue   [!]  |
| [icon] Stream                    |
| [icon] Embeddings                |
|----------------------------------|
| SETTINGS                         |
| [icon] Rate Limiting             |
| [icon] Auth & Users              |
| [icon] Providers                 |
| [icon] LLM Guardrail            |
| [icon] Telemetry                 |
| [icon] Server Config             |
| [icon] Background Tasks          |
|----------------------------------|
| [avatar] admin@acme    [logout]  |
+----------------------------------+
```

**States**:
- **Expanded** (240px): Full text labels, section dividers, count badges
- **Collapsed** (64px): Icons only, tooltips on hover, logo collapses to icon mark
- **Hover**: Item background changes to `gray-100` / `gray-800` (dark)
- **Active**: Item background `primary-50` / `primary-500/10` (dark), left 3px accent bar in `primary-400`, text in `primary-500`
- **Badge**: Notification count pill (e.g., pending approvals) -- `error-500` background with white text

**Tailwind**: `w-60 shrink-0 bg-gray-0 dark:bg-gray-950 border-r border-gray-200 dark:border-gray-800 flex flex-col`

**Accessibility**:
- `<nav aria-label="Main navigation">`
- Each item is `<a>` with `aria-current="page"` for active item
- Collapse button: `aria-label="Collapse sidebar"` / `"Expand sidebar"`
- Keyboard: `Tab` to navigate items, `Enter`/`Space` to activate

---

### Breadcrumbs

**Anatomy**: Horizontal trail showing navigation hierarchy.

```
Dashboard  /  Chains  /  chain-abc-123
```

**Variants**:
- **Standard**: All segments clickable except last (current page)
- **Truncated**: Middle segments collapse to `...` dropdown when > 4 segments

**Tailwind**: `flex items-center gap-1.5 text-sm text-gray-500`
- Separator: `text-gray-300 dark:text-gray-600` using `/` character
- Current: `text-gray-900 dark:text-gray-100 font-medium`

**Accessibility**: `<nav aria-label="Breadcrumb">` with `<ol>` list, `aria-current="page"` on last item.

---

### Tab Group

**Anatomy**: Horizontal tabs for sub-navigation within a view.

```
[ Overview ]  [ Steps ]  [ Configuration ]  [ Logs ]
  --------
```

**States**:
- **Default**: `text-gray-500`, no underline
- **Hover**: `text-gray-700 dark:text-gray-300`
- **Active**: `text-gray-950 dark:text-gray-50`, 2px bottom border in `primary-400`
- **Disabled**: `text-gray-300 dark:text-gray-600 cursor-not-allowed`

**Sizes**:
- **sm**: `text-sm py-1.5 px-3` (used in side panels)
- **md**: `text-base py-2 px-4` (default, used in page views)

**Tailwind**: `flex border-b border-gray-200 dark:border-gray-800`

**Accessibility**: `role="tablist"`, each tab `role="tab"` with `aria-selected`, panels `role="tabpanel"`. Arrow keys navigate between tabs.

---

## Buttons

### Button

**Anatomy**: Clickable element that triggers an action.

```
[ Icon (optional)  Label  ]
```

**Variants**:

| Variant | Default BG | Default Text | Hover BG | Usage |
|---------|-----------|-------------|----------|-------|
| Primary | `primary-400` | `white` | `primary-500` | Main actions (Dispatch, Save) |
| Secondary | `gray-100 dark:gray-800` | `gray-900 dark:gray-100` | `gray-200 dark:gray-700` | Secondary actions (Cancel, Filter) |
| Ghost | `transparent` | `gray-600 dark:gray-400` | `gray-100 dark:gray-800` | Tertiary actions (inline actions) |
| Danger | `error-500` | `white` | `error-700` | Destructive (Drain DLQ, Reject) |
| Success | `success-500` | `white` | `success-700` | Positive actions (Approve) |

**Sizes**:

| Size | Padding | Height | Font Size |
|------|---------|--------|-----------|
| sm | `px-2.5 py-1` | 28px | `text-xs` (11px) |
| md | `px-3 py-1.5` | 34px | `text-sm` (13px) |
| lg | `px-4 py-2` | 40px | `text-base` (14px) |

**States**:
- **Default**: As specified per variant
- **Hover**: Darker background shade
- **Active/Pressed**: Even darker, slight `scale-[0.98]` transform
- **Focus**: `shadow-ring` (2px `primary-400` outline), only on `:focus-visible`
- **Disabled**: `opacity-50 cursor-not-allowed`, no hover effect
- **Loading**: Label replaced with spinner + "Loading..." text, disabled interaction

**Icon-Only Button**: Square dimensions (28/34/40px per size), `aria-label` required.

**Tailwind (Primary md)**: `inline-flex items-center justify-center gap-1.5 px-3 py-1.5 rounded-md bg-primary-400 text-white text-sm font-medium hover:bg-primary-500 focus-visible:ring-2 focus-visible:ring-primary-400 focus-visible:ring-offset-2 transition-fast disabled:opacity-50`

**Accessibility**: Use `<button>` for actions, `<a>` for navigation. `aria-disabled="true"` when disabled. Loading state gets `aria-busy="true"`.

---

## Inputs

### Text Input

**Anatomy**:
```
Label (optional)
+-----------------------------------+
| [icon?]  Placeholder text...      |
+-----------------------------------+
Helper text or error message
```

**States**:
- **Default**: Border `gray-300 dark:gray-600`, bg `gray-0 dark:gray-950`
- **Hover**: Border `gray-400 dark:gray-500`
- **Focus**: Border `primary-400`, `shadow-ring`
- **Error**: Border `error-500`, error message in `error-500` below
- **Disabled**: `bg-gray-50 dark:bg-gray-900 opacity-60`

**Sizes**: sm (28px h), md (34px h), lg (40px h)

**Tailwind**: `w-full px-3 py-1.5 rounded-md border border-gray-300 dark:border-gray-600 bg-gray-0 dark:bg-gray-950 text-sm text-gray-900 dark:text-gray-100 placeholder:text-gray-400 focus:border-primary-400 focus:ring-2 focus:ring-primary-400/20`

**Accessibility**: `<label>` associated via `htmlFor`/`id`. Error state uses `aria-invalid="true"` and `aria-describedby` pointing to error message.

---

### Search Input

**Anatomy**: Text input with magnifying glass icon and optional `Cmd+K` shortcut hint.

```
+----------------------------------------+
| [mag-glass]  Search...       [Cmd+K]  |
+----------------------------------------+
```

**Behavior**: Debounced (300ms) input filtering. `Escape` clears, focuses trigger command palette if empty.

**Tailwind**: Same as text input with `pl-9` for icon inset and `pr-16` for shortcut badge.

---

### Select

**Anatomy**: Dropdown selector for single-value choices.

```
+-----------------------------------+
| Selected value              [v]   |
+-----------------------------------+
| Option 1                         |
| Option 2  (active/highlighted)   |
| Option 3                         |
+-----------------------------------+
```

**States**: Same as text input plus open/closed dropdown state.

**Accessibility**: Uses `Listbox` pattern. `aria-expanded`, `aria-haspopup="listbox"`, arrow keys to navigate, `Enter` to select, `Escape` to close.

---

### Multi-Select

**Anatomy**: Input with multiple selected items as removable chips/pills.

```
+-------------------------------------------+
| [chip: ns1 x] [chip: ns2 x]  Search... |
+-------------------------------------------+
```

**Behavior**: Type to filter, click or Enter to add, `Backspace` to remove last, click `x` to remove specific.

---

### Toggle Switch

**Anatomy**: Binary on/off control (used for rule enable/disable, feature toggles).

```
Off: [    O    ]  (gray background)
On:  [    ===O ]  (primary background)
```

**States**:
- **Off**: `bg-gray-300 dark:bg-gray-600`
- **On**: `bg-primary-400`
- **Disabled**: `opacity-50`

**Tailwind**: `relative inline-flex h-5 w-9 items-center rounded-full transition-fast`

**Accessibility**: `role="switch"`, `aria-checked="true|false"`, `aria-label` describing what it toggles.

---

### Checkbox and Radio

Standard implementations with custom styling. Checkbox uses `rounded-sm`, radio uses `rounded-full`. Both get `shadow-ring` on focus-visible.

---

## Data Display

### Stat Card

**Anatomy**: Compact card showing a single KPI metric with optional sparkline.

```
+----------------------------+
| dispatched        [trend]  |
| 12,847                     |
| [===========__] sparkline  |
| +3.2% vs prev period      |
+----------------------------+
```

**Elements**:
- **Label**: `text-sm text-gray-500 font-medium uppercase tracking-wide`
- **Value**: `text-3xl font-bold text-gray-950 dark:text-gray-50 tabular-nums`
- **Sparkline**: 60x24px SVG line chart, `stroke-primary-400` (or semantic color)
- **Trend**: `text-xs`, green with up arrow for positive, red with down arrow for negative

**Tailwind**: `bg-gray-0 dark:bg-gray-950 border border-gray-200 dark:border-gray-800 rounded-lg p-4`

---

### Badge / Pill

**Anatomy**: Small inline label indicating status or category.

```
[ Executed ]  [ Failed ]  [ Pending ]
```

**Variants** (color mapped to semantic tokens):

| Type | Background | Text | Border |
|------|-----------|------|--------|
| Success | `success-50` / `success-500/10` | `success-700` / `success-300` | none |
| Error | `error-50` / `error-500/10` | `error-700` / `error-300` | none |
| Warning | `warning-50` / `warning-500/10` | `warning-700` / `warning-300` | none |
| Info | `info-50` / `info-500/10` | `info-700` / `info-300` | none |
| Pending | `pending-50` / `pending-500/10` | `pending-700` / `pending-300` | none |
| Neutral | `gray-100` / `gray-800` | `gray-700` / `gray-300` | none |

**Sizes**: `sm` (20px h, text-xs), `md` (24px h, text-sm)

**Tailwind (Success sm)**: `inline-flex items-center px-1.5 py-0.5 rounded-sm text-xs font-medium bg-success-50 text-success-700 dark:bg-success-500/10 dark:text-success-300`

**Accessibility**: If badge conveys status, add `aria-label` with full status description (e.g., `aria-label="Status: Executed successfully"`).

---

### Progress Bar

**Anatomy**: Horizontal fill bar showing completion progress.

```
[===============-------]  65%
```

**Variants**:
- **Default**: `bg-primary-400` fill on `bg-gray-100 dark:bg-gray-800` track
- **Success**: `bg-success-500` fill
- **Error**: `bg-error-500` fill
- **Indeterminate**: Animated shimmer (left-to-right pulse)

**Sizes**: sm (4px h), md (8px h), lg (12px h)

**Accessibility**: `role="progressbar"`, `aria-valuenow`, `aria-valuemin="0"`, `aria-valuemax="100"`, `aria-label`.

---

### Gauge / Meter

**Anatomy**: Semi-circular or circular gauge for cache hit rates, success rates.

```
      ____
    /  85% \
   |        |
    \______/
   Cache Hit Rate
```

**Color zones**: Green (>80%), Yellow (50-80%), Red (<50%) -- configurable thresholds.

**Accessibility**: `role="meter"` with `aria-valuenow`, `aria-valuemin`, `aria-valuemax`.

---

### Timeline / Activity Feed

**Anatomy**: Vertical list of chronological events.

```
O  ActionDispatched - email - ns:prod tenant:acme     10:42:01
|
O  ChainAdvanced - chain-abc step: validate            10:42:02
|
O  Executed - webhook-provider responded 200            10:42:03
```

**Elements**:
- **Dot**: Colored by event type/outcome, 8px circle
- **Line**: 1px `gray-200 dark:gray-700` connecting dots
- **Content**: Event type badge, summary text, timestamp
- **Expandable**: Click to show full event detail

**Behavior**: Auto-scrolls when new events arrive (unless user has scrolled up). "New events" button appears when paused.

---

### JSON Viewer

**Anatomy**: Collapsible tree-view of JSON data with syntax highlighting.

```
v {
    "namespace": "prod",
    "tenant": "acme",
  v "payload": {
      "to": "user@example.com",
      "subject": "Welcome"
    }
  }
```

**Features**:
- Collapse/expand at any level (click `v`/`>`)
- Syntax coloring: keys in `primary-300`, strings in `success-500`, numbers in `warning-500`, booleans in `info-500`, null in `gray-400`
- Copy-to-clipboard button (top right)
- Path display on hover (e.g., `payload.to`)
- Search within JSON (Cmd+F when focused)

**Font**: `JetBrains Mono text-sm`

---

### Code Block

**Anatomy**: Syntax-highlighted code display with language indicator.

```
+-----------------------------------+
| yaml                    [copy]    |
|-----------------------------------|
| name: block-large-emails          |
| priority: 10                      |
| condition:                        |
|   field: payload.size             |
|   operator: gt                    |
|   value: 10485760                 |
| action: deny                      |
+-----------------------------------+
```

**Languages**: YAML, CEL, JSON, TOML

**Tailwind**: `font-mono text-sm bg-gray-50 dark:bg-gray-900 rounded-lg border border-gray-200 dark:border-gray-800 p-4 overflow-x-auto`

---

## Tables

### Data Table

The primary data display component, following Retool's table quality standard.

**Anatomy**:
```
+----------------------------------------------------------------------+
| [Filter chips...]  [Column selector]  [Export]  [Refresh]            |
+----------------------------------------------------------------------+
| # | action_id | namespace | outcome  | provider | dispatched_at      |
|---|-----------|-----------|----------|----------|--------------------|
| 1 | abc-123   | prod      | Executed | webhook  | 2026-02-08 10:42  |
| 2 | def-456   | staging   | Failed   | email    | 2026-02-08 10:41  |
|---|-----------|-----------|----------|----------|--------------------|
| Showing 1-50 of 2,847  [< 1 2 3 ... 57 >]                          |
+----------------------------------------------------------------------+
```

**Features**:
- **Sortable columns**: Click header to sort asc/desc, arrow indicator. Multi-column sort with Shift+click.
- **Column filters**: Per-column dropdown filter (select for enums like outcome, text search for IDs)
- **Row selection**: Checkbox column, select all, bulk actions bar appears
- **Pagination**: Page size selector (25/50/100), page navigation, total count
- **Expandable rows**: Click row or chevron to expand inline detail
- **Empty state**: Illustration + message + CTA (e.g., "No audit records found. Adjust your filters or dispatch an action.")
- **Loading skeleton**: Shimmer rows matching column widths, 3-5 rows
- **Sticky header**: Header row sticks on scroll (`z-sticky`)
- **Row hover**: `bg-gray-50 dark:bg-gray-900`
- **Conditional row coloring**: Failed rows get subtle `error-50` / `error-500/5` background
- **Column resize**: Drag column borders to resize

**Column Types**:
| Type | Rendering |
|------|-----------|
| Text | Left-aligned, truncated with tooltip |
| ID | Monospace (`JetBrains Mono`), copy-on-click |
| Badge | Colored pill (see Badge component) |
| Timestamp | Relative ("2m ago") with absolute on hover |
| Number | Right-aligned, `tabular-nums` |
| Toggle | Inline switch (e.g., rule enabled) |
| Actions | Icon buttons (view, replay, delete) |

**Tailwind**:
- Table: `w-full border-collapse`
- Header: `bg-gray-50 dark:bg-gray-900 text-left text-xs font-semibold text-gray-500 uppercase tracking-wide`
- Cell: `px-3 py-2 text-sm border-b border-gray-100 dark:border-gray-800`
- Row hover: `hover:bg-gray-50 dark:hover:bg-gray-900`

**Accessibility**:
- `<table>` with `<thead>`, `<tbody>`
- Sortable headers: `aria-sort="ascending|descending|none"`, `<button>` inside `<th>`
- Row selection: `aria-selected`, `<input type="checkbox" aria-label="Select row">`
- Pagination: `aria-label="Pagination"`, current page `aria-current="page"`
- Keyboard: Arrow keys for cell navigation when table is focused

---

## Feedback

### Toast Notification

**Anatomy**: Small notification sliding in from top-right.

```
+----------------------------------------+
| [icon]  Action dispatched       [x]    |
|        action_id: abc-123              |
+----------------------------------------+
```

**Severities**:
| Severity | Icon | Left Border | Usage |
|----------|------|-------------|-------|
| Success | Checkmark circle | `success-500` | Action dispatched, rule saved |
| Error | X circle | `error-500` | Dispatch failed, connection lost |
| Warning | Triangle | `warning-500` | Rate limit approaching, circuit half-open |
| Info | Info circle | `info-500` | Rules reloaded, new approval |

**Behavior**:
- Slide in from top-right, stack vertically (max 5 visible)
- Auto-dismiss after 5s (errors persist until dismissed)
- Hover pauses auto-dismiss timer
- Click `x` to dismiss manually
- Swipe right to dismiss (touch)

**Tailwind**: `fixed top-4 right-4 z-toast w-96 bg-gray-0 dark:bg-gray-950 border border-gray-200 dark:border-gray-800 rounded-lg shadow-lg p-4`

**Accessibility**: `role="status"` for info/success, `role="alert"` for error/warning. `aria-live="polite"` (info/success) or `aria-live="assertive"` (error/warning).

---

### Modal Dialog

**Anatomy**: Centered overlay dialog with backdrop.

```
        +-----------------------------------+
        | Dialog Title              [x]     |
        |-----------------------------------|
        | Content area with form or         |
        | confirmation message.             |
        |                                   |
        |-----------------------------------|
        |              [Cancel] [Confirm]   |
        +-----------------------------------+
```

**States**:
- **Open**: Fade-in backdrop (`bg-black/50`), scale+fade dialog
- **Closing**: Reverse animation
- **Focus trapped**: Tab cycles within modal only

**Sizes**: sm (400px), md (500px), lg (640px), xl (768px)

**Tailwind**: `fixed inset-0 z-modal flex items-center justify-center`
- Backdrop: `bg-black/50 backdrop-blur-sm`
- Dialog: `bg-gray-0 dark:bg-gray-950 rounded-xl shadow-lg p-6 w-full max-w-md`

**Accessibility**: `role="dialog"`, `aria-modal="true"`, `aria-labelledby` pointing to title. `Escape` closes. Focus returns to trigger element on close. Focus trapped inside dialog.

---

### Confirmation Dialog

**Anatomy**: Specialized modal for destructive actions.

```
        +-----------------------------------+
        | [warning icon]                    |
        | Drain Dead-Letter Queue?          |
        |                                   |
        | This will permanently remove      |
        | 47 entries. This cannot be undone. |
        |                                   |
        |         [Cancel]  [Drain DLQ]     |
        +-----------------------------------+
```

The destructive action button uses `Danger` variant. User must explicitly click (no Enter shortcut for destructive buttons by default).

---

### Inline Alert

**Anatomy**: Full-width banner within content area.

```
+--------------------------------------------------+
| [icon] Alert message with description.   [close] |
+--------------------------------------------------+
```

**Variants**: Success, Error, Warning, Info -- using corresponding semantic colors for background, border, and icon.

**Tailwind (Error)**: `flex items-start gap-3 p-4 rounded-lg bg-error-50 dark:bg-error-500/10 border border-error-200 dark:border-error-500/20`

---

### Loading Spinner

**Anatomy**: Animated spinning indicator.

**Sizes**: sm (16px), md (20px), lg (24px), xl (32px)

**Behavior**: Obeys the 200ms show-delay / 400ms minimum-display rules.

**Tailwind**: `animate-spin rounded-full border-2 border-gray-200 dark:border-gray-700 border-t-primary-400`

**Accessibility**: `aria-label="Loading"`, `role="status"`.

---

### Skeleton Screen

**Anatomy**: Placeholder shapes matching the content layout, with shimmer animation.

```
+----------------------------------------------------------------------+
| [====shimmer====]  [==shimmer==]  [===shimmer===]  [==shimmer==]     |
| [====shimmer====]  [==shimmer==]  [===shimmer===]  [==shimmer==]     |
| [====shimmer====]  [==shimmer==]  [===shimmer===]  [==shimmer==]     |
+----------------------------------------------------------------------+
```

**Tailwind**: `bg-gray-100 dark:bg-gray-800 rounded animate-pulse`

---

## Layout

### Page Header

**Anatomy**: Top section of each view with title, description, and action buttons.

```
+----------------------------------------------------------------------+
| [breadcrumbs]                                                        |
| Page Title                                [Action 1]  [Action 2]    |
| Optional subtitle or description                                     |
+----------------------------------------------------------------------+
```

**Tailwind**: `flex items-center justify-between pb-6 border-b border-gray-200 dark:border-gray-800`

---

### Card Container

**Anatomy**: Bordered container for grouping related content.

```
+-----------------------------------+
| Card Title            [actions]   |
|-----------------------------------|
| Card content                      |
+-----------------------------------+
```

**Variants**:
- **Default**: `border border-gray-200 dark:border-gray-800 rounded-lg`
- **Elevated**: Add `shadow-md`
- **Interactive**: Add `hover:shadow-md hover:border-gray-300 dark:hover:border-gray-600 cursor-pointer transition-fast`

**Tailwind**: `bg-gray-0 dark:bg-gray-950 border border-gray-200 dark:border-gray-800 rounded-lg p-4`

---

### Drawer / Side Panel

**Anatomy**: Panel sliding in from the right edge for detail views.

```
                                +---------------------------+
                                | Record Detail      [x]   |
                                |---------------------------|
                                | [Tabs: Overview | JSON]  |
                                |                           |
                                | Field: Value              |
                                | Field: Value              |
                                |                           |
                                | [JSON Viewer]             |
                                |                           |
                                |         [Replay] [Close]  |
                                +---------------------------+
```

**Width**: 440px (default), 600px (wide variant for JSON-heavy views)

**Behavior**: Slides from right, backdrop overlay optional (semi-transparent or none for side-by-side browsing). `Escape` closes. Scroll independent of main content.

**Tailwind**: `fixed inset-y-0 right-0 z-overlay w-[440px] bg-gray-0 dark:bg-gray-950 border-l border-gray-200 dark:border-gray-800 shadow-lg transform transition-slow`

**Accessibility**: `role="dialog"` or `role="complementary"`. Focus managed -- trap when overlaid, or let focus flow when side-by-side.

---

### Split Pane

**Anatomy**: Two panels side by side with a draggable divider.

```
+--------------------+---+--------------------+
| Left pane          | | | Right pane         |
| (code editor)      | | | (preview/result)   |
|                    | | |                    |
+--------------------+---+--------------------+
```

**Behavior**: Divider draggable to resize. Double-click to reset to 50/50. Min width per pane: 300px.

---

## Specialized

### Command Palette

**Anatomy**: Cmd+K modal overlay with search and action list.

```
        +-------------------------------------------+
        | [mag-glass]  Type a command or search...   |
        |-------------------------------------------|
        | NAVIGATION                                 |
        |   Dashboard                         Cmd+1 |
        |   Rules                             Cmd+2 |
        |   Chains                            Cmd+3 |
        |                                           |
        | ACTIONS                                    |
        |   Dispatch Action                   Cmd+D |
        |   Reload Rules                      Cmd+R |
        |   Toggle Theme                      Cmd+T |
        |                                           |
        | RECENT                                     |
        |   chain-abc-123                            |
        |   rule: block-large-emails                 |
        +-------------------------------------------+
```

**Features**:
- Fuzzy matching on all items
- Categories: Navigation, Actions, Rules, Chains, Providers, Recent
- Keyboard: Arrow keys navigate, Enter selects, Escape closes
- Shortcut hints right-aligned per item
- Results update as user types (no debounce -- instant)

**Behavior**: `Cmd+K` (Mac) / `Ctrl+K` (Windows/Linux) opens. Typing filters. Press `Backspace` on empty input to go back to root. Recent searches shown on empty input.

**Tailwind**: `fixed inset-0 z-command-palette flex items-start justify-center pt-[20vh]`
- Backdrop: `bg-black/50 backdrop-blur-sm`
- Panel: `w-full max-w-xl bg-gray-0 dark:bg-gray-950 rounded-xl shadow-lg border border-gray-200 dark:border-gray-800 overflow-hidden`

**Accessibility**: `role="combobox"`, `aria-expanded="true"`, `aria-controls` pointing to results list. Results: `role="listbox"`, each item `role="option"`.

---

### Rule Editor

**Anatomy**: Split-pane editor with code on left and preview on right.

```
+-------------------------------+-------------------------------+
| [YAML] [CEL]                  | PARSED RULE PREVIEW           |
|-------------------------------|-------------------------------|
| name: block-large             | Name: block-large             |
| priority: 10                  | Priority: 10                  |
| condition:                    | Condition: payload.size > 10M |
|   field: payload.size         | Action: Deny                  |
|   operator: gt                | Source: YAML                  |
|   value: 10485760             | Enabled: Yes                  |
| action: deny                  |                               |
|                               |-------------------------------|
|                               | DRY-RUN TEST                  |
|                               | [JSON payload input]          |
|                               |         [Run Dry-Run]         |
|                               | Verdict: DENY                 |
|                               | Matched Rule: block-large     |
+-------------------------------+-------------------------------+
```

**Features**:
- Tab switching between YAML and CEL syntax modes
- Syntax highlighting per language
- Line numbers
- Error markers (red squiggly underline on invalid syntax)
- Auto-indentation
- Parsed preview updates in real-time as user types
- Dry-run test panel: paste JSON payload, run against current rule, show verdict

**Font**: `JetBrains Mono text-sm` for both editor and preview code sections.

---

### DAG Visualizer

**Anatomy**: Interactive directed acyclic graph showing chain step flow with branches.

```
    +----------+
    | validate |  (completed, green)
    +----------+
       /     \
      /       \
+--------+  +--------+
| notify |  | reject |  (branch labels on edges)
| (blue) |  | (gray) |
+--------+  +--------+
      \
       \
    +----------+
    |  close   |  (pending, dashed border)
    +----------+
```

**Elements**:
- **Nodes**: Rounded rectangles, 120x50px minimum. Color by status (green=completed, blue=in-progress/active, gray=pending, red=failed, `gray-300` dashed border for skipped).
- **Active node**: Subtle pulse animation (`animate-pulse` with `primary-400` glow).
- **Edges**: SVG paths with arrowheads. Solid for default flow, labeled for branch conditions.
- **Branch labels**: Small pill on edge mid-point showing condition (e.g., `success == true`).
- **Execution path highlight**: The actual executed path gets thicker edges (`stroke-width: 3`) in `primary-400`.
- **Zoom/Pan**: Mouse wheel to zoom, drag to pan. Fit-to-view button.
- **Click node**: Opens side panel with step detail (response, error, timing).

**This is the wow-factor view.** The DAG must look beautiful -- use smooth bezier curves for edges, generous spacing between nodes, and subtle animations for the active execution path.

**Accessibility**: Each node is a focusable button with `aria-label` describing step name and status. Arrow keys navigate between connected nodes.

---

### Circuit Breaker Status Widget

**Anatomy**: Visual state diagram showing the three circuit states with animated transitions.

```
    +--------+     failures >= 5     +---------+
    | CLOSED | ------------------>  |  OPEN   |
    | (green)|                      |  (red)  |
    +--------+                      +---------+
        ^                              |
        |    successes >= 2            | recovery timeout
        |                              v
        +------- +-----------+ <-------+
                 | HALF-OPEN |
                 |  (amber)  |
                 +-----------+
```

**Animation**: When state changes, the active state node scales up briefly (`scale-110`, 200ms), and the transition arrow pulses. Previous state fades to muted color.

**Data**: failure count, success count, current state, time in state, fallback provider (if any).

---

### Live Event Stream Panel

**Anatomy**: Full-height scrolling feed of SSE events with filters.

```
+--------------------------------------------------------------+
| [Connected]  [namespace v] [tenant v] [type v]  [Pause ||]  |
+--------------------------------------------------------------+
| 10:42:03  [ActionDispatched]  email  ns:prod tenant:acme     |
| 10:42:02  [ChainAdvanced]    chain-abc step:validate         |
| 10:42:01  [ApprovalRequired] approval-xyz rule:pii-check     |
| 10:41:58  [Executed]         webhook responded 200           |
| ...                                                          |
+--------------------------------------------------------------+
```

**Features**:
- Connection status indicator: green dot + "Connected", yellow "Reconnecting...", red "Disconnected"
- Auto-scroll to newest (top). Scroll down pauses auto-scroll. "Jump to latest" button appears.
- Pause/Resume button toggles event buffering (events are queued while paused, flushed on resume)
- Filter dropdowns for namespace, tenant, action_type, event_type
- Each event expandable inline for full detail
- Timestamp in `JetBrains Mono`

---

### Approval Action Card

**Anatomy**: Card layout for pending approval items.

```
+--------------------------------------------------------------+
| [PendingApproval badge]                           2m ago     |
|                                                              |
| Action: send-notification (email provider)                   |
| Rule: pii-review-required                                    |
| Message: "Contains PII - requires human review"             |
|                                                              |
| Namespace: prod  |  Tenant: acme  |  Expires: 23m remaining |
|                                                              |
| [v Payload preview]                                          |
|                                                              |
|                            [Reject (red)]  [Approve (green)] |
+--------------------------------------------------------------+
```

**Behavior**:
- Approve/Reject buttons are large and prominent (min 44px touch target)
- Countdown timer for expiration (amber when < 5m, red when < 1m)
- Expandable payload preview (JSON Viewer)
- After action: card transitions to confirmed state with success/rejection badge

**Accessibility**: Buttons have clear `aria-label` ("Approve action send-notification", "Reject action send-notification"). Card is `role="article"` with `aria-label` for the full context.
