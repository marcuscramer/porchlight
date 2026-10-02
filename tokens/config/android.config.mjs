import StyleDictionary from 'style-dictionary';

// Style Dictionary's built-in size/compose/remToDp and size/compose/remToSp
// transforms assume the *source* dimension is expressed in rem and scale it
// by basePxFontSize (16) to get a px-equivalent — see getTokenDimensionValue
// in style-dictionary's own transforms.js. Our token source is authored
// directly in px (matching this project's existing dp≈px convention on
// Android and the web app's own px values), so those built-ins would wrongly
// multiply every value by 16. These two transforms do the same job (append
// .dp/.sp for Compose) without that rem-to-px scaling.
StyleDictionary.registerTransform({
  name: 'size/dp-dtcg',
  type: 'value',
  filter: (token) => token.$type === 'dimension',
  transform: (token) => `(${parseFloat(token.$value)}).dp`,
});

StyleDictionary.registerTransform({
  name: 'size/sp-dtcg',
  type: 'value',
  filter: (token) => token.$type === 'fontSize' || token.$type === 'letterSpacing',
  transform: (token) => `(${parseFloat(token.$value)}).sp`,
});

// Compose alpha params (Color.copy(alpha = ...), etc.) take a Float, and
// Kotlin doesn't implicitly convert a bare Double literal (what `0.14`
// would otherwise generate) to one — needs the `f` suffix.
StyleDictionary.registerTransform({
  name: 'value/float-dtcg',
  type: 'value',
  filter: (token) => token.$type === 'number',
  transform: (token) => `${parseFloat(token.$value)}f`,
});

const packageName = 'dev.porchlight.app.ui.theme';

export default {
  usesDtcg: true,
  source: [],
  include: [
    'primitives/**/*.json',
    'semantic/**/*.json',
    'component/**/*.json',
  ],
  platforms: {
    compose: {
      // color/composeColor is style-dictionary's own built-in (handles our
      // plain "#RRGGBB" hex strings correctly, no custom transform needed).
      transforms: ['attribute/cti', 'name/camel', 'color/composeColor', 'size/dp-dtcg', 'size/sp-dtcg', 'value/float-dtcg'],
      buildPath: '../android-app/app/src/main/java/dev/porchlight/app/ui/theme/',
      files: [
        {
          destination: 'Dimens.kt',
          format: 'compose/object',
          filter: (token) => token.$type === 'dimension',
          options: { className: 'Dimens', packageName },
        },
        {
          destination: 'GeneratedColor.kt',
          format: 'compose/object',
          filter: (token) => token.$type === 'color',
          options: { className: 'GeneratedColor', packageName },
        },
        {
          destination: 'GeneratedType.kt',
          format: 'compose/object',
          // fontFamily is deliberately excluded: Style Dictionary can't
          // construct a Compose FontFamily (no way to reference
          // R.font.inter_display_semibold), so Type.kt keeps that seam
          // hand-written and only needs the numeric scale here.
          // Default import (Color + unit.*) is left as-is — the unit
          // wildcard import covers .sp here, same as Dimens.kt needs .dp;
          // an unused Color import is harmless (Kotlin doesn't warn on
          // unused wildcard imports).
          filter: (token) => ['fontSize', 'fontWeight', 'letterSpacing'].includes(token.$type),
          options: { className: 'GeneratedType', packageName },
        },
        {
          destination: 'GeneratedOpacity.kt',
          format: 'compose/object',
          filter: (token) => token.$type === 'number',
          options: { className: 'GeneratedOpacity', packageName },
        },
      ],
    },
  },
};
