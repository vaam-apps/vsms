// Tailwind 4 ships its own PostCSS plugin; no separate `tailwind.config.js`
// is needed — all configuration lives in app/globals.css and
// frontends/packages/ui/src/styles/theme.css via `@plugin`/`@theme` directives.
const config = {
  plugins: {
    "@tailwindcss/postcss": {},
  },
};

export default config;
