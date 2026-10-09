#!/usr/bin/env node

// Servidor de desenvolvimento para o frontend do aic chat.
// - Serve arquivos de src/chat/web diretamente do disco.
// - Live Reload configurável via LIVE_RELOAD=true|false.
// - Faz proxy das chamadas de API para o backend aic.
// - Inicia o backend automaticamente, caso não esteja rodando.

const http = require('http');
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const PORT = parseInt(process.env.PORT || '3000', 10);
const WEB_DIR = path.resolve(__dirname, '../src/chat/web');
const REPO_ROOT = path.resolve(__dirname, '..');

// Live Reload habilitado por padrão.
// Para desabilitar: LIVE_RELOAD=false node scripts/dev-server.js
const LIVE_RELOAD = process.env.LIVE_RELOAD !== 'false';

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
  '.json': 'application/json',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2'
};

// -----------------------------------------------------------------------------
// Backend
// -----------------------------------------------------------------------------

function getBackendTarget() {
  try {
    const lockPath = path.join(REPO_ROOT, '.aic/chat/server.lock');

    if (fs.existsSync(lockPath)) {
      const lock = fs.readFileSync(lockPath, 'utf8');
      const match = lock.match(/http:\/\/127\.0\.0\.1:(\d+)/);

      if (match) {
        return {
          host: '127.0.0.1',
          port: parseInt(match[1], 10)
        };
      }
    }
  } catch {}

  return {
    host: '127.0.0.1',
    port: 7878
  };
}

function checkBackendAlive(target) {
  return new Promise((resolve) => {
    const req = http.get(
      `http://${target.host}:${target.port}/api/agents`,
      { timeout: 1000 },
      (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      }
    );

    req.on('error', () => resolve(false));

    req.on('timeout', () => {
      req.destroy();
      resolve(false);
    });
  });
}

function findAicBinary() {
  const localAic = path.join(
    process.env.HOME || '',
    '.local/bin/aic'
  );

  if (fs.existsSync(localAic)) {
    return localAic;
  }

  return 'aic';
}

async function ensureBackend() {
  let target = getBackendTarget();

  if (await checkBackendAlive(target)) {
    console.log(
      `✔ Backend aic já em execução em http://${target.host}:${target.port}`
    );
    return null;
  }

  const aicBin = findAicBinary();

  console.log(
    `Iniciando backend aic (${aicBin} chat --no-open)...`
  );

  const child = spawn(aicBin, ['chat', '--no-open'], {
    cwd: REPO_ROOT,
    stdio: ['ignore', 'inherit', 'inherit']
  });

  child.on('error', (err) => {
    console.error(`Falha ao iniciar ${aicBin}:`, err.message);
  });

  for (let i = 0; i < 30; i++) {
    await new Promise((resolve) => setTimeout(resolve, 200));

    target = getBackendTarget();

    if (await checkBackendAlive(target)) {
      console.log(
        `✔ Backend aic conectado em http://${target.host}:${target.port}`
      );
      break;
    }
  }

  return child;
}

// -----------------------------------------------------------------------------
// Live Reload - Server-Sent Events
// -----------------------------------------------------------------------------

const reloadClients = new Set();

let reloadTimer = null;
let fileWatcher = null;

function notifyReload(filename) {
  if (!LIVE_RELOAD) return;

  clearTimeout(reloadTimer);

  reloadTimer = setTimeout(() => {
    console.log(`🔄 Reload: ${filename}`);

    for (const client of reloadClients) {
      try {
        client.write('data: reload\n\n');
      } catch {
        reloadClients.delete(client);
      }
    }
  }, 150);
}

function watchFiles() {
  if (!LIVE_RELOAD) return;

  try {
    fileWatcher = fs.watch(
      WEB_DIR,
      { recursive: true },
      (eventType, filename) => {
        if (!filename) {
          notifyReload('unknown');
          return;
        }

        console.log(`📝 ${eventType}: ${filename}`);

        notifyReload(filename);
      }
    );

    fileWatcher.on('error', (err) => {
      console.error('Erro no file watcher:', err.message);
    });

    console.log(`👀 Monitorando alterações em: ${WEB_DIR}`);
  } catch (err) {
    console.error(
      'Não foi possível iniciar o Live Reload:',
      err.message
    );
  }
}

function handleReloadConnection(req, res) {
  if (!LIVE_RELOAD) {
    res.writeHead(404, {
      'Content-Type': 'text/plain; charset=utf-8'
    });
    res.end('Live Reload disabled');
    return;
  }

  res.writeHead(200, {
    'Content-Type': 'text/event-stream; charset=utf-8',
    'Cache-Control': 'no-cache, no-transform',
    'Connection': 'keep-alive',
    'X-Accel-Buffering': 'no'
  });

  res.write(': connected\n\n');

  reloadClients.add(res);

  const heartbeat = setInterval(() => {
    if (!res.destroyed) {
      res.write(': heartbeat\n\n');
    }
  }, 30000);

  req.on('close', () => {
    clearInterval(heartbeat);
    reloadClients.delete(res);
  });
}

// -----------------------------------------------------------------------------
// Live Reload - Browser Script
// -----------------------------------------------------------------------------

const LIVE_RELOAD_SCRIPT = `
<script>
  (() => {
    if (window.__AIC_LIVE_RELOAD__) return;

    window.__AIC_LIVE_RELOAD__ = true;

    const events = new EventSource('/__dev/reload');

    events.onopen = () => {
      console.log('[Live Reload] Connected');
    };

    events.onmessage = (event) => {
      if (event.data === 'reload') {
        console.log('[Live Reload] Reloading...');
        window.location.reload();
      }
    };

    events.onerror = () => {
      console.warn('[Live Reload] Connection lost. Reconnecting...');
    };
  })();
</script>
`;

function injectLiveReload(html) {
  if (!LIVE_RELOAD) return html;

  if (/<\/body\s*>/i.test(html)) {
    return html.replace(
      /<\/body\s*>/i,
      `${LIVE_RELOAD_SCRIPT}</body>`
    );
  }

  return html + LIVE_RELOAD_SCRIPT;
}

// -----------------------------------------------------------------------------
// Static Files
// -----------------------------------------------------------------------------

function serveStaticFile(req, res, filePath) {
  const ext = path.extname(filePath).toLowerCase();

  const headers = {
    'Content-Type': MIME_TYPES[ext] || 'application/octet-stream',
    'Cache-Control': 'no-cache, no-store, must-revalidate'
  };

  if (ext === '.html') {
    fs.readFile(filePath, 'utf8', (err, html) => {
      if (err) {
        res.writeHead(500, {
          'Content-Type': 'text/plain; charset=utf-8'
        });
        res.end('Erro ao ler arquivo HTML.');
        return;
      }

      const modifiedHtml = LIVE_RELOAD
        ? injectLiveReload(html)
        : html;

      res.writeHead(200, headers);
      res.end(req.method === 'HEAD' ? undefined : modifiedHtml);
    });

    return;
  }

  res.writeHead(200, headers);

  if (req.method === 'HEAD') {
    res.end();
    return;
  }

  const stream = fs.createReadStream(filePath);

  stream.on('error', (err) => {
    console.error(`Erro ao servir ${filePath}:`, err.message);
    res.destroy(err);
  });

  stream.pipe(res);
}

// -----------------------------------------------------------------------------
// Backend Proxy
// -----------------------------------------------------------------------------

function proxyToBackend(req, res) {
  const target = getBackendTarget();

  const headers = {
    ...req.headers,
    host: `${target.host}:${target.port}`
  };

  if (headers.origin) {
    headers.origin = `http://${target.host}:${target.port}`;
  }

  const proxyReq = http.request(
    {
      hostname: target.host,
      port: target.port,
      path: req.url,
      method: req.method,
      headers
    },
    (proxyRes) => {
      res.writeHead(proxyRes.statusCode, proxyRes.headers);
      proxyRes.pipe(res);
    }
  );

  proxyReq.on('error', (err) => {
    if (res.headersSent) {
      res.destroy(err);
      return;
    }

    res.writeHead(502, {
      'Content-Type': 'text/plain; charset=utf-8'
    });

    res.end(
      `Não foi possível conectar ao backend aic em ` +
      `http://${target.host}:${target.port}.\n` +
      `Erro: ${err.message}`
    );
  });

  req.pipe(proxyReq);
}

// -----------------------------------------------------------------------------
// HTTP Server
// -----------------------------------------------------------------------------

async function start() {
  let aicProcess = null;

  try {
    aicProcess = await ensureBackend();
  } catch (err) {
    console.warn(
      'Aviso: Não foi possível verificar/iniciar o backend:',
      err.message
    );
  }

  const server = http.createServer((req, res) => {
    let urlPath;

    try {
      urlPath = new URL(
        req.url,
        `http://${req.headers.host || 'localhost'}`
      ).pathname;

      urlPath = decodeURIComponent(urlPath);
    } catch {
      res.writeHead(400);
      res.end('Invalid URL');
      return;
    }

    // Live Reload SSE endpoint
    if (urlPath === '/__dev/reload') {
      if (!LIVE_RELOAD) {
        res.writeHead(404);
        res.end('Live Reload disabled');
        return;
      }

      if (req.method !== 'GET') {
        res.writeHead(405, { Allow: 'GET' });
        res.end();
        return;
      }

      handleReloadConnection(req, res);
      return;
    }

    // Static files
    const filename = urlPath === '/'
      ? 'chat.html'
      : urlPath.replace(/^\/+/, '');

    const filePath = path.resolve(WEB_DIR, filename);

    // Prevent directory traversal
    const relativePath = path.relative(WEB_DIR, filePath);

    if (
      relativePath === '..' ||
      relativePath.startsWith(`..${path.sep}`) ||
      path.isAbsolute(relativePath)
    ) {
      res.writeHead(403);
      res.end('Forbidden');
      return;
    }

    if (req.method === 'GET' || req.method === 'HEAD') {
      try {
        if (
          fs.existsSync(filePath) &&
          fs.statSync(filePath).isFile()
        ) {
          serveStaticFile(req, res, filePath);
          return;
        }
      } catch (err) {
        console.error('Erro ao acessar arquivo:', err.message);
      }
    }

    // Proxy everything else to backend
    proxyToBackend(req, res);
  });

  // ---------------------------------------------------------------------------
  // Cleanup
  // ---------------------------------------------------------------------------

  const cleanup = () => {
    console.log('\nEncerrando servidor de desenvolvimento...');

    clearTimeout(reloadTimer);

    if (fileWatcher) {
      fileWatcher.close();
    }

    for (const client of reloadClients) {
      client.end();
    }

    reloadClients.clear();

    server.close();

    if (aicProcess && !aicProcess.killed) {
      console.log('Encerrando backend aic...');
      aicProcess.kill();
    }

    process.exit(0);
  };

  process.on('SIGINT', cleanup);
  process.on('SIGTERM', cleanup);

  // ---------------------------------------------------------------------------
  // Start listening
  // ---------------------------------------------------------------------------

  server.listen(PORT, '127.0.0.1', () => {
    const target = getBackendTarget();

    if (LIVE_RELOAD) {
      watchFiles();
    }

    console.log('\n----------------------------------------');
    console.log('🚀 AIC Chat Development Server');
    console.log('----------------------------------------');
    console.log(`🌐 Frontend: http://127.0.0.1:${PORT}`);
    console.log(`📁 Web directory: ${WEB_DIR}`);
    console.log(`🔗 Backend: http://${target.host}:${target.port}`);
    console.log(`⚡ Live Reload: ${LIVE_RELOAD ? 'enabled' : 'disabled'}`);
    console.log('----------------------------------------\n');
  });
}

start().catch((err) => {
  console.error('Erro ao iniciar servidor:', err);
  process.exit(1);
});