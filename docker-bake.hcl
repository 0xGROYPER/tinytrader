# Every service image from the Dockerfile, in one build (the workspace
# compiles once for all of them):
#   docker buildx bake --load                 # all, as tinytrader/<service>:local
#   docker buildx bake --load ticker console  # some
# CI (.github/workflows/images.yml) sets the variables below to publish them
# to ghcr.io/<owner>/tinytrader-<service>.

# Image name: this prefix and the service name.
variable "IMAGE_PREFIX" {
  default = "tinytrader/"
}
# Comma-separated tags for every image.
variable "TAGS" {
  default = "local"
}
# OCI labels: the source repository URL (which also links a GitHub package
# to its repository) and the commit.
variable "SOURCE" {
  default = ""
}
variable "REVISION" {
  default = ""
}
# A GitHub Actions cache scope; empty for none.
variable "GHA_CACHE" {
  default = ""
}

variable "SERVICES" {
  default = [
    "ticker",
    "console",
    "relay",
    "store",
    "databento-jobs",
    "alpaca-news",
    "alpaca-account",
    "alpaca-data",
    "alpaca-history",
    "econ-calendar",
    "order-gateway",
    "queue",
  ]
}

group "default" {
  targets = SERVICES
}

target "service" {
  name       = svc
  matrix     = { svc = SERVICES }
  context    = "."
  dockerfile = "Dockerfile"
  target     = "service"
  args       = { SERVICE = svc }
  tags       = [for t in split(",", TAGS) : "${IMAGE_PREFIX}${svc}:${t}"]
  labels = merge(
    SOURCE == "" ? {} : { "org.opencontainers.image.source" = SOURCE },
    REVISION == "" ? {} : { "org.opencontainers.image.revision" = REVISION },
  )
  cache-from = GHA_CACHE == "" ? [] : ["type=gha,scope=${GHA_CACHE}"]
  cache-to   = GHA_CACHE == "" ? [] : ["type=gha,scope=${GHA_CACHE},mode=max"]
}
