{ pkgs ? import <nixpkgs> {} }:

let
  helper = 1;
  acquire = helper;
in
  { inherit acquire; }
