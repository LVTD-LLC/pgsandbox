#!/bin/sh
set -eu

usage() {
  printf 'usage: %s FORMULA_PATH VERSION SHA256\n' "$0" >&2
}

if [ "$#" -ne 3 ]; then
  usage
  exit 2
fi

formula_path=$1
version=$2
sha256=$3

case "$version" in
  v*)
    printf 'version must not include leading v: %s\n' "$version" >&2
    exit 1
    ;;
  *[!0-9A-Za-z.+-]* | '')
    printf 'version contains unsupported characters: %s\n' "$version" >&2
    exit 1
    ;;
esac

if [ ! -f "$formula_path" ]; then
  printf 'formula not found: %s\n' "$formula_path" >&2
  exit 1
fi

if [ "${#sha256}" -ne 64 ]; then
  printf 'sha256 must be 64 hex characters\n' >&2
  exit 1
fi

case "$sha256" in
  *[!0-9a-fA-F]*)
    printf 'sha256 must be hexadecimal\n' >&2
    exit 1
    ;;
esac

release_url="https://github.com/LVTD-LLC/pgsandbox-mcp/releases/download/v${version}/pgsandbox-mcp-${version}.tar.gz"

FORMULA_PATH=$formula_path RELEASE_URL=$release_url RELEASE_SHA256=$sha256 perl <<'PERL'
use strict;
use warnings;

my $path = $ENV{FORMULA_PATH};
my $release_url = $ENV{RELEASE_URL};
my $release_sha256 = lc $ENV{RELEASE_SHA256};

open my $input, '<', $path or die "could not read $path: $!\n";
local $/;
my $formula = <$input>;
close $input;

my $url_count = ($formula =~ s{^([ \t]*)url\s+["'][^"']+["'][ \t]*$}{$1url "$release_url"}m);
my $sha_count = ($formula =~ s{^([ \t]*)sha256\s+["'][0-9a-fA-F]+["'][ \t]*$}{$1sha256 "$release_sha256"}m);

die "could not find url line in $path\n" unless $url_count;
die "could not find sha256 line in $path\n" unless $sha_count;

open my $output, '>', $path or die "could not write $path: $!\n";
print {$output} $formula;
close $output;
PERL

printf 'updated %s\n' "$formula_path"
printf 'url:    %s\n' "$release_url"
printf 'sha256: %s\n' "$(printf '%s' "$sha256" | tr 'A-F' 'a-f')"
