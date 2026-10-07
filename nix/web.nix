# checks.<system>.console-types: type-check the console's page. The page is
# plain JavaScript served as written (no build step); TypeScript checks it
# against its JSDoc annotations and services/console/types.
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
    };
}
