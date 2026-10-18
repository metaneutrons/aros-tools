#ifndef ORDER
#error "ordered contract lost final definition"
#endif
#if ORDER != 1
#error "ordered contract changed final value"
#endif
#ifndef LANGUAGE_CXX
#error "CXX contract missing"
#endif
#ifdef LANGUAGE_C
#error "C contract leaked into CXX"
#endif
static_assert(sizeof(VALUE) == 4, "quoted whitespace changed");
static_assert(sizeof(APOSTROPHE) == 4, "quoted apostrophe changed");
int ordered_contract_cxx() { return ORDER; }
