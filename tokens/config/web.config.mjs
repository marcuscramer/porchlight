export default {
  usesDtcg: true,
  include: [
    'primitives/**/*.json',
    'semantic/**/*.json',
    'component/**/*.json',
  ],
  // Source overrides include, by design (no "already defined" warning) —
  // this is where the two genuinely web-specific values live: the Inter
  // font-family fallback stack, and the hover-brightness token TV has no
  // use for at all.
  source: ['platform-overrides/web.json'],
  platforms: {
    css: {
      transformGroup: 'css',
      buildPath: '../web-app/',
      files: [
        {
          destination: 'tokens.css',
          format: 'css/variables',
          options: {
            outputReferences: true,
          },
        },
      ],
    },
  },
};
