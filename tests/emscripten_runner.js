const http = require("http");
const fs = require("fs");
const { opendir } = require("node:fs/promises");

const { loadPyodide } = require("./pyodide/pyodide.js");

const PORT = 8124;

const server = http
  .createServer(function (request, response) {
    const filePath = "." + request.url;
    const contentType = "application/octet-stream";
    fs.readFile(
      process.argv[2] + "/target/wheels/" + filePath,
      function (error, content) {
        if (error) {
          if (error.code == "ENOENT") {
            response.writeHead(404);
            response.end("Not found");
            response.end();
          } else {
            response.writeHead(500);
            response.end("error: " + error.code);
            response.end();
          }
        } else {
          response.writeHead(200, { "Content-Type": contentType });
          response.end(content, "utf-8");
        }
      }
    );
  })
  .listen(PORT);

async function findWheel(distDir) {
  const dir = await opendir(distDir);
  for await (const dirent of dir) {
    if (dirent.name.endsWith("whl")) {
      return dirent.name;
    }
  }
}

const localhost = `http://0.0.0.0:${PORT}`;
const pkgDir = process.argv[2];
const distDir = pkgDir + "/target/wheels";
const testDir = pkgDir + "/tests";

async function main() {
  const wheelName = await findWheel(distDir);
  const wheelURL = `${localhost}/${wheelName}`;

  let errcode = 1;
  try {
    pyodide = await loadPyodide({ indexURL: "./pyodide", fullStdLib: false });
    pyodide._api.setCdnUrl("https://pyodide-cdn2.iodide.io/dev/full/");
    const FS = pyodide.FS;
    const NODEFS = FS.filesystems.NODEFS;
    FS.mkdir("/test_dir");
    FS.mount(NODEFS, { root: testDir }, "/test_dir");
    await pyodide.loadPackage(["micropip", "pytest", "tomli"]);
    const micropip = pyodide.pyimport("micropip");
    await micropip.install(wheelURL);
    const pytest = pyodide.pyimport("pytest");
    errcode = pytest.main(pyodide.toPy(["/test_dir", "-vv"]));
  } catch (e) {
    console.error(e);
    errcode = 1;
  } finally {
    server.close();
  }
  process.exit(errcode);
}

main();
