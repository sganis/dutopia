# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
npm install          # Install dependencies
npm run dev          # Development server (port 5173)
npm run build        # Production build
npm run check        # TypeScript check
npm run preview      # Preview production build
```

## Tech Stack

- **Framework**: SvelteKit 2 + Svelte 5 (runes: $state, $derived)
- **CSS**: TailwindCSS 4
- **Build**: Vite 6
- **Adapter**: Static (SPA mode, SSR disabled)

## Project Structure

```
src/
├── lib/              # Reusable components (PascalCase)
│   ├── Login.svelte
│   ├── TreeMap.svelte
│   └── Picker*.svelte
├── routes/           # SvelteKit pages
│   ├── +layout.svelte
│   └── +page.svelte  # Main dashboard (NEEDS REFACTOR)
└── ts/               # TypeScript modules (lowercase)
    ├── api.svelte.ts # API client with $state
    ├── cache.ts      # IndexedDB cache (1min TTL)
    ├── store.svelte.ts # Global state
    └── util.ts       # Formatting, colors, paths
```

## Naming Conventions

- **Components**: PascalCase (`Login.svelte`, `TreeMap.svelte`)
- **Modules/Utils**: lowercase (`api.svelte.ts`, `util.ts`)
- **Routes**: SvelteKit convention (`+page.svelte`, `+layout.svelte`)

## Key Types

```typescript
type FolderItem = {
  path: string;
  total_count: number;
  total_disk: number;
  total_size: number;
  total_linked: number;
  accessed: number;
  modified: number;
  users: Record<string, UserStatsJson>;
}

type AgeFilter = -1 | 0 | 1 | 2;  // -1=all, 0=recent, 1=mid, 2=old
```

## API Endpoints

```
POST /api/login           # Returns JWT
GET  /api/users           # List usernames
GET  /api/folders?path=&users=&age=  # Folder stats
GET  /api/files?path=&users=&age=    # File list
```

## State Management

Uses Svelte 5 runes:
- `$state()` for reactive state
- `$derived.by()` for computed values
- `SvelteMap` for reactive maps

## File Size Status

Files requiring refactoring (> 600 line limit):
- `+page.svelte`: 1,079 lines → extract components:
  - Tooltip logic
  - Age filter dropdown
  - Sort dropdown
  - Folder/file list items
  - Path input with breadcrumb

## Dependencies

- `svelecte`: Dropdown component
- `svelte-awesome-color-picker`: Color picker
- `date-fns`: Date formatting
- `idb`: IndexedDB wrapper
- `material-symbols`: Icons
