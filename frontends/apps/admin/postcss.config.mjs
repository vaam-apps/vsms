// Tailwind 4 ships its own PostCSS plugin; no separate `tailwind.config.js`
// is needed — all configuration lives in app/globals.css and the theme
// stylesheet it imports from @vaam-apps/ui, via `@plugin`/`@theme`
// directives.
const config = {
  plugins: {
    "@tailwindcss/postcss": {},
  },
};

export default config;
