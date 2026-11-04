// Lightweight zero-dependency local dev server for GTT Desktop UI testing
const http = require('http');
const fs = require('fs');
const path = require('path');

let port = parseInt(process.env.PORT || process.argv[2] || '5173', 10);
const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
};

const server = http.createServer((req, res) => {
  let reqPath = req.url.split('?')[0];
  if (reqPath === '/' || reqPath === '') {
    reqPath = '/index.html';
  }

  const safePath = path.normalize(reqPath).replace(/^(\.\.[\/\\])+/, '');
  const filePath = path.join(__dirname, safePath);

  fs.stat(filePath, (err, stats) => {
    if (err || !stats.isFile()) {
      res.writeHead(404, { 'Content-Type': 'text/plain' });
      res.end('404 Not Found');
      return;
    }

    const ext = path.extname(filePath).toLowerCase();
    const contentType = MIME_TYPES[ext] || 'application/octet-stream';

    res.writeHead(200, { 'Content-Type': contentType });
    fs.createReadStream(filePath).pipe(res);
  });
});

function listen(p) {
  server.once('error', (err) => {
    if (err.code === 'EADDRINUSE') {
      console.log(`Port ${p} is in use, trying port ${p + 1}...`);
      listen(p + 1);
    } else {
      console.error('Server error:', err);
    }
  });

  server.listen(p, '127.0.0.1', () => {
    console.log(`\n======================================================`);
    console.log(` GTT Desktop App Preview Server Running!              `);
    console.log(` Open in your browser: http://127.0.0.1:${p}          `);
    console.log(`======================================================\n`);
  });
}

listen(port);
