const fs = require("node:fs");
const { copyFile, mkdtemp, opendir } = require("node:fs/promises");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { loadPyodide } = require("pyodide");

async function findWheel(distDir) {
  // Match every Emscripten / Pyodide platform tag family in the wheel
  // filename's *platform tag* field. Wheel filenames are
  // `{name}-{ver}-{python tag}-{abi tag}-{platform tag}.whl`, so anchor to
  // the trailing `-{platform tag}.whl` segment to avoid false positives
  // from a project name that happens to contain `pyodide_` etc.
  // - `pyemscripten_<year>_<patch>_wasm32` (PEP 783)
  // - `pyodide_<year>_<patch>_wasm32`     (legacy pre-PEP 783 naming)
  // - `emscripten_<emcc-version>_wasm32`  (legacy, Pyodide <= 0.27)
  const tagRegex = /-(pyemscripten|pyodide|emscripten)_[^-]+_wasm32\.whl$/;
  const dir = await opendir(distDir);
  for await (const dirent of dir) {
    if (tagRegex.test(dirent.name)) {
      return dirent.name;
    }
  }
}

async function wheelPathForRuntime(distDir, wheelName, pyodide) {
  const pep783PlatformVersion = pyodide.runPython(`
import sysconfig
sysconfig.get_config_var("PYEMSCRIPTEN_PLATFORM_VERSION")
`);
  if (pep783PlatformVersion) {
    return `${distDir}/${wheelName}`;
  }

  // Pyodide 0.28/0.29 runtimes still validate wheel filenames using the
  // pre-PEP 783 `pyodide_<year>_<patch>_wasm32` spelling. Maturin should build
  // the PyPI-accepted `pyemscripten_*` filename, but for the runtime smoke test
  // we need a compatibility copy with the old platform-tag spelling.
  const legacyName = wheelName.replace(
    /-pyemscripten_(\d+_\d+)_wasm32\.whl$/,
    "-pyodide_$1_wasm32.whl",
  );
  if (legacyName === wheelName) {
    return `${distDir}/${wheelName}`;
  }
  const runtimeDir = await mkdtemp(join(tmpdir(), "maturin-pyodide-runtime-"));
  const runtimePath = `${runtimeDir}/${legacyName}`;
  await copyFile(`${distDir}/${wheelName}`, runtimePath);
  return runtimePath;
}

function make_tty_ops(stream){
  return {
    // get_char has 3 particular return values:
    // a.) the next character represented as an integer
    // b.) undefined to signal that no data is currently available
    // c.) null to signal an EOF
    get_char(tty) {
      if (!tty.input.length) {
        var result = null;
        var BUFSIZE = 256;
        var buf = Buffer.alloc(BUFSIZE);
        var bytesRead = fs.readSync(process.stdin.fd, buf, 0, BUFSIZE, -1);
        if (bytesRead === 0) {
          return null;
        }
        result = buf.slice(0, bytesRead);
        tty.input = Array.from(result);
      }
      return tty.input.shift();
    },
    put_char(tty, val) {
      try {
        if(val !== null){
          tty.output.push(val);
        }
        if (val === null || val === 10) {
          process.stdout.write(Buffer.from(tty.output));
          tty.output = [];
        }
      } catch(e){
        console.warn(e);
      }
    },
    fsync(tty) {
      if (!tty.output || tty.output.length === 0) {
        return;
      }
      stream.write(Buffer.from(tty.output));
      tty.output = [];
    }
  };
}

function setupStreams(FS, TTY){
  let mytty = FS.makedev(FS.createDevice.major++, 0);
  let myttyerr = FS.makedev(FS.createDevice.major++, 0);
  TTY.register(mytty, make_tty_ops(process.stdout))
  TTY.register(myttyerr, make_tty_ops(process.stderr))
  FS.mkdev('/dev/mytty', mytty);
  FS.mkdev('/dev/myttyerr', myttyerr);
  FS.unlink('/dev/stdin');
  FS.unlink('/dev/stdout');
  FS.unlink('/dev/stderr');
  FS.symlink('/dev/mytty', '/dev/stdin');
  FS.symlink('/dev/mytty', '/dev/stdout');
  FS.symlink('/dev/myttyerr', '/dev/stderr');
  FS.closeStream(0);
  FS.closeStream(1);
  FS.closeStream(2);
  var stdin = FS.open('/dev/stdin', 0);
  var stdout = FS.open('/dev/stdout', 1);
  var stderr = FS.open('/dev/stderr', 1);
}

const pkgDir = process.argv[2];
const distDir = pkgDir + "/target/wheels";
const testDir = pkgDir + "/tests";

async function main() {
  const wheelName = await findWheel(distDir);

  try {
    pyodide = await loadPyodide();
    const wheelPath = await wheelPathForRuntime(distDir, wheelName, pyodide);
    const wheelURL = `file:${wheelPath}`;
    const FS = pyodide.FS;
    setupStreams(FS, pyodide._module.TTY);
    const NODEFS = FS.filesystems.NODEFS;
    FS.mkdir("/test_dir");
    FS.mount(NODEFS, { root: testDir }, "/test_dir");
    await pyodide.loadPackage(["micropip", "pytest", "tomli"]);
    const micropip = pyodide.pyimport("micropip");
    await micropip.install(wheelURL);
    const pytest = pyodide.pyimport("pytest");
    FS.chdir("/test_dir");
    errcode = pytest.main();
  } catch (e) {
    console.error(e);
    process.exit(1);
  }
}

main();
