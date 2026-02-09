# Acteon Admin UI -- Design System

> The foundational design tokens for building every screen of the Acteon Admin UI.
> All values are implementation-ready for a React + Tailwind CSS stack.

---

## Table of Contents

1. [Color System](#color-system)
2. [Typography](#typography)
3. [Spacing](#spacing)
4. [Border Radius](#border-radius)
5. [Shadows](#shadows)
6. [Transitions](#transitions)
7. [Z-Index Scale](#z-index-scale)

---

## Color System

Colors are defined in the **LCH color space** for perceptual uniformity (following Linear's approach), then converted to HEX/HSL for implementation. The palette uses **luminosity reversal** for dark mode (following Cloudflare's technique): light-mode gray-50 becomes dark-mode gray-900 and vice versa, preserving brand identity with minimal manual overrides.

### Neutral Scale (12 steps)

Used for backgrounds, surfaces, borders, and text.

| Token | Light Mode | Dark Mode | HSL (Light) | Usage |
|-------|-----------|-----------|-------------|-------|
| `gray-950` | `#0A0A0B` | `#FAFAFA` | `240 7% 4%` | Primary text (light) |
| `gray-900` | `#131316` | `#F5F5F6` | `240 10% 8%` | Headings (light) |
| `gray-800` | `#1E1E23` | `#EBEBED` | `240 10% 13%` | Secondary text (light) |
| `gray-700` | `#2E2E36` | `#DCDCE0` | `240 10% 20%` | Tertiary text (light) |
| `gray-600` | `#46464F` | `#C4C4CC` | `240 6% 30%` | Muted text |
| `gray-500` | `#64647A` | `#A3A3B3` | `240 10% 44%` | Placeholder text |
| `gray-400` | `#8B8BA3` | `#8B8BA3` | `240 12% 59%` | Disabled text |
| `gray-300` | `#B4B4C7` | `#64647A` | `240 18% 74%` | Borders |
| `gray-200` | `#D6D6E1` | `#46464F` | `240 18% 85%` | Subtle borders |
| `gray-100` | `#EBEBF0` | `#2E2E36` | `240 18% 93%` | Hover surfaces |
| `gray-50` | `#F5F5F8` | `#1E1E23` | `240 24% 96%` | Secondary surface |
| `gray-0` | `#FFFFFF` | `#131316` | `0 0% 100%` | Primary surface |

**Dark-mode background**: `#131316` (off-black, not pure `#000000` -- per Cloudflare research, users find off-black less harsh and more readable).

### Primary Brand Color

Acteon's primary is a muted indigo-violet, evoking intelligence and control.

| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `primary-50` | `#EEF0FF` | `230 100% 97%` | Tinted backgrounds |
| `primary-100` | `#D9DEFF` | `232 100% 92%` | Hover tint |
| `primary-200` | `#B3BEFF` | `232 100% 85%` | Active tint |
| `primary-300` | `#8090FF` | `230 100% 75%` | Accents, links |
| `primary-400` | `#5C6FFF` | `232 100% 68%` | Primary buttons, active states |
| `primary-500` | `#4254DB` | `232 64% 56%` | Darker button press |

### Semantic Colors

Each semantic color has 5 shades: `50` (tint/bg), `100` (lighter), `300` (icon/badge bg), `500` (main), `700` (text on light).

#### Success (Green)
| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `success-50` | `#ECFDF5` | `152 76% 96%` | Success background |
| `success-100` | `#D1FAE5` | `149 80% 90%` | Success hover |
| `success-300` | `#6EE7B7` | `160 64% 67%` | Badge background |
| `success-500` | `#10B981` | `160 84% 39%` | Success icon, text |
| `success-700` | `#047857` | `162 93% 24%` | Success text on light bg |

#### Error (Red)
| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `error-50` | `#FEF2F2` | `0 86% 97%` | Error background |
| `error-100` | `#FEE2E2` | `0 93% 94%` | Error hover |
| `error-300` | `#FCA5A5` | `0 94% 82%` | Badge background |
| `error-500` | `#EF4444` | `0 84% 60%` | Error icon, text |
| `error-700` | `#B91C1C` | `0 72% 42%` | Error text on light bg |

#### Warning (Amber)
| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `warning-50` | `#FFFBEB` | `48 100% 96%` | Warning background |
| `warning-100` | `#FEF3C7` | `44 97% 89%` | Warning hover |
| `warning-300` | `#FCD34D` | `46 97% 65%` | Badge background |
| `warning-500` | `#F59E0B` | `38 92% 50%` | Warning icon, text |
| `warning-700` | `#B45309` | `32 81% 39%` | Warning text on light bg |

#### Info (Blue)
| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `info-50` | `#EFF6FF` | `214 100% 97%` | Info background |
| `info-100` | `#DBEAFE` | `214 95% 93%` | Info hover |
| `info-300` | `#93C5FD` | `212 96% 78%` | Badge background |
| `info-500` | `#3B82F6` | `217 91% 60%` | Info icon, text |
| `info-700` | `#1D4ED8` | `224 76% 48%` | Info text on light bg |

#### Pending (Purple)
| Token | HEX | HSL | Usage |
|-------|-----|-----|-------|
| `pending-50` | `#F5F3FF` | `254 100% 97%` | Pending background |
| `pending-100` | `#EDE9FE` | `253 89% 95%` | Pending hover |
| `pending-300` | `#C4B5FD` | `253 86% 85%` | Badge background |
| `pending-500` | `#8B5CF6` | `259 80% 66%` | Pending icon, text |
| `pending-700` | `#6D28D9` | `263 70% 50%` | Pending text on light bg |

### Circuit Breaker Status Colors

| State | Token | Light HEX | Dark HEX | Icon |
|-------|-------|-----------|----------|------|
| Closed (healthy) | `circuit-closed` | `#10B981` | `#34D399` | Filled circle |
| Open (rejecting) | `circuit-open` | `#EF4444` | `#F87171` | Open circle with X |
| Half-Open (testing) | `circuit-half-open` | `#F59E0B` | `#FBBF24` | Half-filled circle |

### Provider Health Colors

| State | Token | Light HEX | Dark HEX |
|-------|-------|-----------|----------|
| Healthy | `provider-healthy` | `#10B981` | `#34D399` |
| Degraded | `provider-degraded` | `#F59E0B` | `#FBBF24` |
| Down | `provider-down` | `#EF4444` | `#F87171` |

### Action Outcome Colors

| Outcome | Token | HEX | Usage |
|---------|-------|-----|-------|
| Executed | `outcome-executed` | `#10B981` | Success green |
| Deduplicated | `outcome-deduplicated` | `#64647A` | Muted gray |
| Suppressed | `outcome-suppressed` | `#EF4444` | Blocked red |
| Rerouted | `outcome-rerouted` | `#3B82F6` | Info blue |
| Throttled | `outcome-throttled` | `#F59E0B` | Warning amber |
| Failed | `outcome-failed` | `#DC2626` | Deep red |
| Grouped | `outcome-grouped` | `#8B5CF6` | Purple |
| PendingApproval | `outcome-pending` | `#F59E0B` | Amber |
| ChainStarted | `outcome-chain` | `#3B82F6` | Blue |
| DryRun | `outcome-dryrun` | `#64647A` | Gray |
| CircuitOpen | `outcome-circuit` | `#EF4444` | Red |
| Scheduled | `outcome-scheduled` | `#8B5CF6` | Purple |

### Data Visualization Palette (8-Color Diverging, Colorblind-Safe)

Designed to be distinguishable under protanopia, deuteranopia, and tritanopia. Based on the Wong palette recommendations.

| Index | Name | HEX | Usage |
|-------|------|-----|-------|
| 1 | Blue | `#0077BB` | Primary series |
| 2 | Orange | `#EE7733` | Secondary series |
| 3 | Cyan | `#33BBEE` | Tertiary series |
| 4 | Magenta | `#CC3366` | Fourth series |
| 5 | Teal | `#009988` | Fifth series |
| 6 | Yellow | `#CCBB44` | Sixth series |
| 7 | Grey | `#BBBBBB` | Seventh series |
| 8 | Red-Purple | `#AA3377` | Eighth series |

Never rely on color alone -- always pair with pattern, shape, label, or icon.

### Dark Mode Implementation

Token-based approach using CSS custom properties:

```css
:root {
  /* Surface tokens (swap in dark mode) */
  --surface-primary: var(--gray-0);       /* #FFFFFF -> #131316 */
  --surface-secondary: var(--gray-50);    /* #F5F5F8 -> #1E1E23 */
  --surface-tertiary: var(--gray-100);    /* #EBEBF0 -> #2E2E36 */

  /* Text tokens (swap in dark mode) */
  --text-primary: var(--gray-950);        /* #0A0A0B -> #FAFAFA */
  --text-secondary: var(--gray-700);      /* #2E2E36 -> #DCDCE0 */
  --text-tertiary: var(--gray-500);       /* #64647A -> #A3A3B3 */

  /* Border tokens */
  --border-default: var(--gray-200);      /* #D6D6E1 -> #46464F */
  --border-subtle: var(--gray-100);       /* #EBEBF0 -> #2E2E36 */
}
```

System preference detection:
```css
@media (prefers-color-scheme: dark) { /* apply dark tokens */ }
```

Manual toggle persisted to `localStorage`.

---

## Typography

### Font Families

| Usage | Family | Fallback Stack | Notes |
|-------|--------|---------------|-------|
| UI / Sans | **Inter** | `-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` | Inter Display for headings >= 20px |
| Code / Mono | **JetBrains Mono** | `"SF Mono", "Fira Code", "Cascadia Code", monospace` | For action types, rule names, IDs, payloads, code editors |

Monospace is used for all technical identifiers: action IDs, chain IDs, namespaces, tenants, rule conditions, JSON payloads, CEL expressions, fingerprints, timestamps.

### Type Scale

| Token | Size (px) | Size (rem) | Line Height | Weight | Usage |
|-------|-----------|-----------|-------------|--------|-------|
| `text-xs` | 11 | 0.6875 | 16px (1.45) | regular (400) | Auxiliary labels, timestamps in dense views |
| `text-sm` | 13 | 0.8125 | 20px (1.54) | regular (400) | Table cells, secondary text, badges |
| `text-base` | 14 | 0.875 | 22px (1.57) | regular (400) | Body text, form labels, sidebar items |
| `text-md` | 15 | 0.9375 | 24px (1.6) | medium (500) | Emphasized body, stat labels |
| `text-lg` | 17 | 1.0625 | 26px (1.53) | semibold (600) | Card titles, section headers |
| `text-xl` | 20 | 1.25 | 28px (1.4) | semibold (600) | Page subtitles (use Inter Display) |
| `text-2xl` | 24 | 1.5 | 32px (1.33) | bold (700) | Page titles (use Inter Display) |
| `text-3xl` | 30 | 1.875 | 38px (1.27) | bold (700) | Dashboard hero stat numbers (use Inter Display) |

### Font Weights

| Token | Value | Usage |
|-------|-------|-------|
| `font-regular` | 400 | Body, table cells |
| `font-medium` | 500 | Labels, sidebar active items |
| `font-semibold` | 600 | Section headers, card titles |
| `font-bold` | 700 | Page titles, stat values |

### Letter Spacing

| Token | Value | Usage |
|-------|-------|-------|
| `tracking-tight` | -0.01em | Headings text-xl and above |
| `tracking-normal` | 0em | Body text |
| `tracking-wide` | 0.02em | Uppercase labels, badge text |

### Special Typography Rules

- **Tabular numbers**: All numeric values in tables use `font-variant-numeric: tabular-nums` for column alignment.
- **Monospace for IDs**: Action IDs, chain IDs, and fingerprints always render in `JetBrains Mono` at `text-sm`.
- **Truncation**: Long values (payloads, descriptions) truncate with `...` and expand on hover/click.
- **Inter Display**: Used for `text-xl` (20px) and above -- provides tighter, more distinctive headings.

---

## Spacing

Base unit: **4px**. All spacing derives from this base.

| Token | Value | Pixels | Usage |
|-------|-------|--------|-------|
| `space-0` | 0 | 0px | Reset |
| `space-0.5` | 0.125rem | 2px | Tight inline spacing (badge padding) |
| `space-1` | 0.25rem | 4px | Inline icon-text gap |
| `space-1.5` | 0.375rem | 6px | Compact element spacing |
| `space-2` | 0.5rem | 8px | Tight padding (table cells, small buttons) |
| `space-3` | 0.75rem | 12px | Default button padding-x, input padding |
| `space-4` | 1rem | 16px | Card padding, form group gap |
| `space-5` | 1.25rem | 20px | Section gap within cards |
| `space-6` | 1.5rem | 24px | Content padding, page margin |
| `space-8` | 2rem | 32px | Section dividers |
| `space-10` | 2.5rem | 40px | Large section gaps |
| `space-12` | 3rem | 48px | Page section spacing |
| `space-16` | 4rem | 64px | Major layout gaps |
| `space-20` | 5rem | 80px | Hero spacing |

### Layout Spacing Conventions

| Context | Spacing |
|---------|---------|
| Table cell padding | `space-2` (8px) vertical, `space-3` (12px) horizontal |
| Card internal padding | `space-4` (16px) to `space-6` (24px) |
| Form field gap | `space-4` (16px) |
| Sidebar item padding | `space-2` (8px) vertical, `space-3` (12px) horizontal |
| Page content padding | `space-6` (24px) |
| Stat card value to label | `space-1` (4px) |

---

## Border Radius

| Token | Value | Usage |
|-------|-------|-------|
| `radius-none` | 0px | Flat edges (table rows in dense mode) |
| `radius-sm` | 4px | Badges, pills, small chips |
| `radius-md` | 6px | Buttons, inputs, dropdown items |
| `radius-lg` | 8px | Cards, panels, modals |
| `radius-xl` | 12px | Large cards, dialog boxes |
| `radius-full` | 9999px | Avatars, toggle switches, circular indicators |

---

## Shadows

Light mode shadows use gray tones; dark mode uses a darker ambient.

| Token | Light Mode | Dark Mode | Usage |
|-------|-----------|-----------|-------|
| `shadow-sm` | `0 1px 2px rgba(0,0,0,0.05)` | `0 1px 2px rgba(0,0,0,0.3)` | Subtle lift: buttons, badges |
| `shadow-md` | `0 4px 6px -1px rgba(0,0,0,0.07), 0 2px 4px -1px rgba(0,0,0,0.04)` | `0 4px 6px -1px rgba(0,0,0,0.4), 0 2px 4px -1px rgba(0,0,0,0.2)` | Cards, dropdown menus |
| `shadow-lg` | `0 10px 15px -3px rgba(0,0,0,0.08), 0 4px 6px -2px rgba(0,0,0,0.03)` | `0 10px 15px -3px rgba(0,0,0,0.5), 0 4px 6px -2px rgba(0,0,0,0.2)` | Modals, popovers, command palette |
| `shadow-ring` | `0 0 0 2px var(--primary-400)` | `0 0 0 2px var(--primary-300)` | Focus ring (`:focus-visible`) |

---

## Transitions

All transitions use `cubic-bezier(0.16, 1, 0.3, 1)` (ease-out curve, smooth deceleration).

| Token | Duration | Easing | Usage |
|-------|----------|--------|-------|
| `transition-fast` | 150ms | ease-out | Button hover, toggle switch, icon rotation |
| `transition-base` | 200ms | ease-out | Page transitions, color changes, focus rings |
| `transition-slow` | 300ms | ease-out | Side panel slide-in, modal fade, drawer open |

### Easing Curves

| Token | Value | Usage |
|-------|-------|-------|
| `ease-out` | `cubic-bezier(0.16, 1, 0.3, 1)` | Default for enters/transitions |
| `ease-in` | `cubic-bezier(0.7, 0, 0.84, 0)` | Exits and closings |
| `ease-in-out` | `cubic-bezier(0.45, 0, 0.55, 1)` | Looping/repeating animations (shimmer) |
| `spring` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | Slight overshoot (command palette open) |

### Spinner / Loading Timing

Following Vercel's micro-interaction research:
- **Show delay**: 200ms -- do not show spinner until 200ms has passed (prevents flicker for fast operations)
- **Minimum display**: 400ms -- once shown, display for at least 400ms (prevents disorienting flash)
- **Optimistic update**: Apply changes immediately and reconcile on server response

---

## Z-Index Scale

Layered scale to prevent z-index wars.

| Token | Value | Usage |
|-------|-------|-------|
| `z-base` | 0 | Default content |
| `z-dropdown` | 100 | Dropdown menus, select popovers |
| `z-sticky` | 200 | Sticky table headers, sidebar |
| `z-overlay` | 300 | Side panel overlay, backdrop |
| `z-modal` | 400 | Modal dialogs, confirmation dialogs |
| `z-toast` | 500 | Toast notifications (always above modals) |
| `z-command-palette` | 600 | Command palette (highest -- always accessible) |

---

## Implementation Notes

### CSS Custom Properties Strategy

All tokens are expressed as CSS custom properties on `:root` and overridden under `.dark` or `@media (prefers-color-scheme: dark)`. Tailwind CSS uses `darkMode: 'class'` for manual toggle support.

### APCA Contrast Targets

Following Vercel's recommendation for APCA (Accessible Perceptual Contrast Algorithm) over WCAG 2:
- **Body text (14px regular)**: Lc 60 minimum
- **Large text (18px+ bold or 24px+)**: Lc 45 minimum
- **Non-text elements (icons, borders)**: Lc 30 minimum
- **Placeholder text**: Lc 40 minimum

### Colorblindness Validation

Following Cloudflare's approach, test all status colors and data visualization palettes with simulation filters for:
- Protanopia, Deuteranopia, Tritanopia (dichromacy)
- Protanomaly, Deuteranomaly, Tritanomaly (anomalous trichromacy)
- Achromatopsia, Achromatomaly (monochromacy)

Every status color is paired with a distinct icon and text label so no information is conveyed by color alone.
