# Sample Nix file to exercise syntax highlighting, indents,
# and textobjects in vorto. Open with `vorto assets/samples/hello.nix`.

{ lib
, stdenv
, fetchFromGitHub
, pkg-config
, openssl
, withFeatureA ? true
, withFeatureB ? false
}:

let
  pname = "vorto-sample";
  version = "0.1.0";

  greeting = name: "Hello, ${name}!";

  features = lib.optionals withFeatureA [ "feature-a" ]
          ++ lib.optionals withFeatureB [ "feature-b" ];

  numbers = lib.range 1 5;

  squared = builtins.map (n: n * n) numbers;

  classify = n:
    if n < 0 then "negative"
    else if n == 0 then "zero"
    else if lib.mod n 2 == 0 then "positive even"
    else "positive odd";

  person = {
    name = "Alice";
    age = 30;
    tags = [ "admin" "early-bird" ];
  };

  multiline = ''
    This is a multiline string.
    Interpolation works: ${greeting person.name}
    And so does ''${escapes}.
  '';
in

stdenv.mkDerivation rec {
  inherit pname version;

  src = fetchFromGitHub {
    owner = "example";
    repo = pname;
    rev = "v${version}";
    sha256 = lib.fakeSha256;
  };

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ openssl ];

  cargoBuildFlags = lib.concatStringsSep " "
    (builtins.map (f: "--features=${f}") features);

  meta = with lib; {
    description = "Sample derivation for vorto Nix highlighting";
    homepage = "https://example.com/${pname}";
    license = licenses.mit;
    platforms = platforms.unix;
    longDescription = multiline;
  };
}
