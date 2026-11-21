import { defineConfig } from 'vite';

/**
 * Builds optional Vite dev/preview proxy rules when `GATEWAY` (or `VITE_GATEWAY`)
 * is set, mirroring Traefik ingress routing without failing offline E2E runs.
 */
function resolveGatewayProxy() {
  const rawGateway = (process.env.GATEWAY || process.env.VITE_GATEWAY || '').trim();
  if (!rawGateway) {
    return undefined;
  }

  const villageAdminPort = process.env.VILLAGE_ADMIN_PORT || '9091';
  const asteroidAdminPort = process.env.ASTEROID_ADMIN_PORT || '9092';
  const villageWsPort = process.env.VILLAGE_WS_PORT || '8443';
  const asteroidWsPort = process.env.ASTEROID_WS_PORT || '8444';

  const normalizedAlias =
    rawGateway === '1' || rawGateway === 'true' || rawGateway === 'local'
      ? '127.0.0.1'
      : rawGateway;

  let parsed = null;
  if (/^https?:\/\//i.test(normalizedAlias)) {
    try {
      parsed = new URL(normalizedAlias);
    } catch {
      parsed = null;
    }
  }

  // Remote ingress URL (e.g. GATEWAY=https://rust-server.kousa.dev): forward subpaths directly
  if (parsed && parsed.hostname !== '127.0.0.1' && parsed.hostname !== 'localhost') {
    const httpTarget = parsed.origin;
    const wsTarget = `${parsed.protocol === 'https:' ? 'wss:' : 'ws:'}//${parsed.host}`;
    return {
      '/cert-hash': { target: httpTarget, changeOrigin: true },
      '/village/cert-hash': { target: httpTarget, changeOrigin: true },
      '/asteroid/cert-hash': { target: httpTarget, changeOrigin: true },
      '/ws': { target: wsTarget, ws: true, changeOrigin: true },
      '/village/ws': { target: wsTarget, ws: true, changeOrigin: true },
      '/asteroid/ws': { target: wsTarget, ws: true, changeOrigin: true },
    };
  }

  // Local / LAN host: map subpath prefixes to per-game gateway ports
  const host = parsed ? parsed.hostname : normalizedAlias.replace(/\/+$/, '');
  return {
    '/village/cert-hash': {
      target: `http://${host}:${villageAdminPort}`,
      changeOrigin: true,
      rewrite: (path) => path.replace(/^\/village/, ''),
    },
    '/cert-hash': {
      target: `http://${host}:${villageAdminPort}`,
      changeOrigin: true,
    },
    '/village/ws': {
      target: `ws://${host}:${villageWsPort}`,
      ws: true,
      changeOrigin: true,
      rewrite: (path) => path.replace(/^\/village/, ''),
    },
    '/asteroid/cert-hash': {
      target: `http://${host}:${asteroidAdminPort}`,
      changeOrigin: true,
      rewrite: (path) => path.replace(/^\/asteroid/, ''),
    },
    '/asteroid/ws': {
      target: `ws://${host}:${asteroidWsPort}`,
      ws: true,
      changeOrigin: true,
      rewrite: (path) => path.replace(/^\/asteroid/, ''),
    },
  };
}

/**
 * Strips `/village` or `/asteroid` prefixes from static/module requests in dev/preview
 * so relative imports work identically on `/village/` and `/asteroid/` with trailing slashes.
 */
function subpathStaticRewritePlugin() {
  const rewriteSubpathAssets = (req, _res, next) => {
    if (req.url) {
      const match = req.url.match(
        /^\/(?:village|asteroid)(\/(?:src|assets|node_modules|@vite|@fs|@id)\/.*|\/style\.css|\/favicon\.ico)(\?.*)?$/
      );
      if (match) {
        req.url = `${match[1]}${match[2] || ''}`;
      }
    }
    next();
  };

  return {
    name: 'mmo-subpath-static-rewrite',
    configureServer(server) {
      server.middlewares.use(rewriteSubpathAssets);
    },
    configurePreviewServer(server) {
      server.middlewares.use(rewriteSubpathAssets);
    },
  };
}

const gatewayProxy = resolveGatewayProxy();

export default defineConfig({
  base: '/',
  plugins: [subpathStaticRewritePlugin()],
  server: {
    port: Number(process.env.VITE_PORT || 3000),
    proxy: gatewayProxy,
  },
  preview: {
    port: Number(process.env.VITE_PREVIEW_PORT || 4173),
    proxy: gatewayProxy,
  },
  build: {
    target: 'esnext',
    rollupOptions: {
      output: {
        manualChunks: {
          three: ['three'],
        },
      },
    },
  },
  test: {
    include: ['src/**/*.test.js'],
    exclude: ['e2e/**', 'node_modules/**', 'dist/**'],
    environment: 'node',
    unstubGlobals: true,
  },
});
