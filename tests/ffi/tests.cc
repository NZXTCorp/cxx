#include <numeric>
#include "tests/ffi/tests.h"
#include "tests/ffi/lib.rs.h"
#include <cstring>
#include <stdexcept>

extern "C" void cxx_test_suite_set_correct() noexcept;
extern "C" tests::R *cxx_test_suite_get_box() noexcept;
extern "C" bool cxx_test_suite_r_is_correct(const tests::R *) noexcept;

namespace tests {

C::C(size_t n) : n(n) {}

size_t C::get() const { return this->n; }

size_t c_return_primitive() { return 2020; }

Shared c_return_shared() { return Shared{2020}; }

rust::Box<R> c_return_box() {
  return rust::Box<R>::from_raw(cxx_test_suite_get_box());
}

std::unique_ptr<C> c_return_unique_ptr() {
  return std::unique_ptr<C>(new C{2020});
}

const size_t &c_return_ref(const Shared &shared) { return shared.z; }

rust::Str c_return_str(const Shared &shared) {
  (void)shared;
  return "2020";
}

rust::String c_return_rust_string() { return "2020"; }

std::unique_ptr<std::string> c_return_unique_ptr_string() {
  return std::unique_ptr<std::string>(new std::string("2020"));
}

std::unique_ptr<std::vector<uint8_t>> c_return_unique_ptr_vector_u8() {
  auto retval = std::unique_ptr<std::vector<uint8_t>>(new std::vector<uint8_t>());
  retval->push_back(86);
  retval->push_back(75);
  retval->push_back(30);
  retval->push_back(9);
  return retval;
}

std::unique_ptr<std::vector<Shared>> c_return_unique_ptr_vector_shared() {
  auto retval = std::unique_ptr<std::vector<Shared>>(new std::vector<Shared>());
  retval->push_back(Shared{1010});
  retval->push_back(Shared{1011});
  return retval;
}

void c_take_primitive(size_t n) {
  if (n == 2020) {
    cxx_test_suite_set_correct();
  }
}

void c_take_shared(Shared shared) {
  if (shared.z == 2020) {
    cxx_test_suite_set_correct();
  }
}

void c_take_box(rust::Box<R> r) {
  if (cxx_test_suite_r_is_correct(&*r)) {
    cxx_test_suite_set_correct();
  }
}

void c_take_unique_ptr(std::unique_ptr<C> c) {
  if (c->get() == 2020) {
    cxx_test_suite_set_correct();
  }
}

void c_take_ref_r(const R &r) {
  if (cxx_test_suite_r_is_correct(&r)) {
    cxx_test_suite_set_correct();
  }
}

void c_take_ref_c(const C &c) {
  if (c.get() == 2020) {
    cxx_test_suite_set_correct();
  }
}

void c_take_str(rust::Str s) {
  if (std::string(s) == "2020") {
    cxx_test_suite_set_correct();
  }
}

void c_take_rust_string(rust::String s) {
  if (std::string(s) == "2020") {
    cxx_test_suite_set_correct();
  }
}

void c_take_unique_ptr_string(std::unique_ptr<std::string> s) {
  if (*s == "2020") {
    cxx_test_suite_set_correct();
  }
}

void c_take_unique_ptr_vector_u8(std::unique_ptr<std::vector<uint8_t>> v) {
  if (v->size() == 4) {
    cxx_test_suite_set_correct();
  }
}

void c_take_unique_ptr_vector_shared(std::unique_ptr<std::vector<Shared>> v) {
  if (v->size() == 2) {
    cxx_test_suite_set_correct();
  }
}

void c_take_vec_u8(const ::rust::Vec<uint8_t>& v) {
  auto cv = static_cast<std::vector<uint8_t>>(v);
  uint8_t sum = std::accumulate(cv.begin(), cv.end(), 0);
  if (sum == 200) {
    cxx_test_suite_set_correct();
  }
}

void c_take_vec_shared(const ::rust::Vec<Shared>& v) {
  auto cv = static_cast<std::vector<Shared>>(v);
  uint32_t sum = 0;
  for (auto i: cv) {
    sum += i.z;
  }
  if (sum == 2021) {
    cxx_test_suite_set_correct();
  }
}

void c_try_return_void() {}

size_t c_try_return_primitive() { return 2020; }

size_t c_fail_return_primitive() { throw std::logic_error("logic error"); }

std::unique_ptr<std::string> c_try_return_string() {
  return std::unique_ptr<std::string>(new std::string("ok"));
}

std::unique_ptr<std::string> c_fail_return_string() { 
  throw std::logic_error("logic error getting string"); 
}

extern "C" C *cxx_test_suite_get_unique_ptr() noexcept {
  return std::unique_ptr<C>(new C{2020}).release();
}

extern "C" std::string *cxx_test_suite_get_unique_ptr_string() noexcept {
  return std::unique_ptr<std::string>(new std::string("2020")).release();
}

extern "C" const char *cxx_run_test() noexcept {
#define STRINGIFY(x) #x
#define TOSTRING(x) STRINGIFY(x)
#define ASSERT(x)                                                              \
  do {                                                                         \
    if (!(x)) {                                                                \
      return "Assertion failed: `" #x "`, " __FILE__ ":" TOSTRING(__LINE__);   \
    }                                                                          \
  } while (false)

  ASSERT(r_return_primitive() == 2020);
  ASSERT(r_return_shared().z == 2020);
  ASSERT(cxx_test_suite_r_is_correct(&*r_return_box()));
  ASSERT(r_return_unique_ptr()->get() == 2020);
  ASSERT(r_return_ref(Shared{2020}) == 2020);
  ASSERT(std::string(r_return_str(Shared{2020})) == "2020");
  ASSERT(std::string(r_return_rust_string()) == "2020");
  ASSERT(*r_return_unique_ptr_string() == "2020");

  r_take_primitive(2020);
  r_take_shared(Shared{2020});
  r_take_unique_ptr(std::unique_ptr<C>(new C{2020}));
  r_take_ref_c(C{2020});
  r_take_str(rust::Str("2020"));
  r_take_rust_string(rust::String("2020"));
  r_take_unique_ptr_string(
      std::unique_ptr<std::string>(new std::string("2020")));

  ASSERT(r_try_return_primitive() == 2020);
  try {
    r_fail_return_primitive();
    ASSERT(false);
  } catch (const rust::Error &e) {
    ASSERT(std::strcmp(e.what(), "rust error") == 0);
  }

  cxx_test_suite_set_correct();
  return nullptr;
}

} // namespace tests
