# checks.<system>.console-types: type-check the console's page. The page is
# plain JavaScript served as written (no build step); TypeScript checks it
# against its JSDoc annotations and services/console/types.
# checks.<system>.console-js: the page's Node tests (services/console/tests/*.test.mjs).
{ ... }:
{
  perSystem =
    { pkgs, lib, ... }:
    let
      console = ../services/console;
      src = lib.fileset.toSource {
        root = console;
        fileset = lib.fileset.unions [
          (console + "/tsconfig.json")
          (console + "/types")
          (console + "/web/lib")
          (console + "/web/widgets")
          (lib.fileset.fileFilter (f: lib.hasSuffix ".test.mjs" f.name) (console + "/tests"))
        ];
      };
    in
    {
      checks.console-types = pkgs.runCommand "tinytrader-console-types" {
        nativeBuildInputs = [ pkgs.typescript ];
      } ''
        cd ${src}
        tsc -p .
        touch $out
      '';
      checks.console-js = pkgs.runCommand "tinytrader-console-js" {
        nativeBuildInputs = [ pkgs.nodejs ];
      } ''
        cd ${src}
        node --test tests/*.test.mjs
        touch $out
      '';
    };
}
