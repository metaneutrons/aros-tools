cmake_minimum_required(VERSION 3.22)
project(SfdHeaderFixture NONE)
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source")
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SDK/include")
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/tools/sfdc" "${AROS_SOURCE_DIR}/unit"
    "${AROS_SDK_INCLUDE_DIR}")
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/Makefile" [=[
include $(TOP)/config/make.cfg
MAIN = main.pl
CLASSES = $(wildcard [A-Z]*.pl)
PODS = SFDCompile.pod
all : $(SFDC)
$(SFDC) : $(MAIN) $(CLASSES) $(PODS)
@cat $^ | $(SED) "s/SFDC_VERSION/$(VERSION)/g" | $(SED) "s/SFDC_DATE/$(DATE)/g" >$@
@$(CHMOD) a+x $@
clean:
@$(RM) -f $(SFDC)
]=])
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/configure.in" "VERSION=1.3\nDATE=2004-11-12\n")
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/main.pl" [=[#!/usr/bin/perl -w
use strict;
my ($out, $mode);
foreach my $arg (@ARGV) {
  $out = $1 if $arg =~ /^--output=(.*)$/;
  $mode = $1 if $arg =~ /^--mode=(.*)$/;
}
exit 0 if -e "$ARGV[-1].no-output";
open my $f, '>', $out or die $!;
print $f "header $mode\n";
close $f or die $!;
]=])
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/CLib.pl" "# uppercase class\n")
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/lower.pl" "die 'lowercase file must not enter the tool';\n")
file(WRITE "${AROS_SOURCE_DIR}/tools/sfdc/SFDCompile.pod" "# documentation\n")
file(WRITE "${AROS_SOURCE_DIR}/unit/mmakefile.src" "# sealed recipe fixture\n")
file(WRITE "${AROS_SOURCE_DIR}/unit/example.sfd" "fixture\n")
file(SHA256 "${AROS_SOURCE_DIR}/unit/mmakefile.src" _recipe_hash)
file(SHA256 "${AROS_SOURCE_DIR}/unit/example.sfd" _input_hash)
set(_job "clib|x-aros|unit/example.sfd|${_input_hash}|clib/example.h")
if(CASE STREQUAL "bad-mode")
    set(_job "shell|x-aros|unit/example.sfd|${_input_hash}|clib/example.h")
elseif(CASE STREQUAL "traversal")
    set(_job "clib|x-aros|unit/example.sfd|${_input_hash}|../escaped.h")
elseif(CASE STREQUAL "extra-field")
    string(APPEND _job "|extra")
elseif(CASE STREQUAL "input-symlink")
    file(RENAME "${AROS_SOURCE_DIR}/unit/example.sfd" "${AROS_SOURCE_DIR}/unit/original.sfd")
    file(CREATE_LINK "original.sfd" "${AROS_SOURCE_DIR}/unit/example.sfd" SYMBOLIC)
elseif(CASE STREQUAL "recipe-drift")
    file(APPEND "${AROS_SOURCE_DIR}/tools/sfdc/Makefile" "@echo extra\n")
elseif(CASE STREQUAL "output-symlink")
    file(CREATE_LINK "source" "${AROS_SDK_INCLUDE_DIR}/clib" SYMBOLIC)
endif()
include("${ENGINE_DIR}/SfdHeaders.cmake")
aros_generate_sfd_headers(NAME fixture-headers FILE unit/mmakefile.src
    FILE_SHA256 "${_recipe_hash}" JOBS "${_job}")
if(CASE STREQUAL "duplicate-output")
    aros_generate_sfd_headers(NAME another-owner FILE unit/mmakefile.src
        FILE_SHA256 "${_recipe_hash}" JOBS "${_job}")
endif()
