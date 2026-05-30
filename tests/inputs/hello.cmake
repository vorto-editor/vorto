# Sample CMake file to exercise syntax highlighting and indents in
# vorto. Open with `vorto assets/samples/hello.cmake`.

#[[
  Block comment: this script demonstrates typical CMake constructs —
  variables, functions, conditionals, loops, generator expressions,
  and target definitions.
]]

cmake_minimum_required(VERSION 3.20)
project(hello VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

option(HELLO_ENABLE_TESTS "Build the test suite" ON)

function(add_greeting target)
  cmake_parse_arguments(ARG "QUIET" "PREFIX" "SOURCES" ${ARGN})
  if(NOT ARG_PREFIX)
    set(ARG_PREFIX "Hello")
  endif()
  add_library(${target} ${ARG_SOURCES})
  target_compile_definitions(${target} PRIVATE GREETING="${ARG_PREFIX}")
endfunction()

set(SOURCES src/main.cpp src/greeter.cpp)

foreach(src IN LISTS SOURCES)
  message(STATUS "source: ${src}")
endforeach()

add_executable(hello ${SOURCES})
target_include_directories(hello PUBLIC include)
target_compile_options(hello PRIVATE
  $<$<CONFIG:Debug>:-g -O0>
  $<$<CONFIG:Release>:-O3>
)

if(HELLO_ENABLE_TESTS)
  enable_testing()
  add_test(NAME smoke COMMAND hello --version)
endif()
