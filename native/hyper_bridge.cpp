// Owned bridge to the official Tableau Hyper C++ SDK. No vendor code is copied.
// Only SELECT queries reach executeQuery. The database argument is a disposable
// snapshot created by Rust; we never connect the SDK to the user's source file.
#include <hyperapi/hyperapi.hpp>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iomanip>
#include <limits>
#include <locale>
#include <sstream>
#include <stdexcept>
#include <string>
#include <unordered_map>

namespace {
class BoundedBuffer final : public std::streambuf {
    std::size_t limit_;
public:
    std::string data;
    explicit BoundedBuffer(std::size_t limit) : limit_(limit) {}
    std::streamsize xsputn(const char* s, std::streamsize n) override {
        if (n < 0 || static_cast<std::size_t>(n) > limit_ - data.size())
            throw std::length_error("result limit");
        data.append(s, static_cast<std::size_t>(n)); return n;
    }
    int overflow(int c) override {
        if (c == traits_type::eof()) return traits_type::not_eof(c);
        char ch = static_cast<char>(c); xsputn(&ch, 1); return c;
    }
};
template<class T> std::string rendered(const T& value, std::size_t limit) {
    BoundedBuffer b(limit); std::ostream s(&b);
    s.exceptions(std::ios::badbit | std::ios::failbit);
    s.imbue(std::locale::classic()); s << std::setprecision(std::numeric_limits<double>::max_digits10) << value;
    return b.data;
}
void json_string(std::ostream& out, const std::string& s) {
    static const char hex[] = "0123456789abcdef";
    out << '"';
    for (unsigned char c : s) {
        if (c == '"' || c == '\\') out << '\\' << static_cast<char>(c);
        else if (c < 0x20) out << "\\u00" << hex[c >> 4] << hex[c & 15];
        else out << static_cast<char>(c);
    }
    out << '"';
}
std::string literal(const char* input) {
    std::string s="'";
    for (const char* p=input;*p;++p) { if (*p=='\'') s.push_back('\'');s.push_back(*p); }
    return s+"'";
}
char* owned(const std::string& s) noexcept {
    char* p=static_cast<char*>(std::malloc(s.size()+1));
    if (p) std::memcpy(p,s.c_str(),s.size()+1);return p;
}
std::string run(const char* runtime,const char* memory,const char* snapshot,const char* sql,
                const char* const* schemas,const char* const* tables,std::size_t table_count,
                std::size_t max_rows,std::size_t max_bytes) {
    hyperapi::HyperProcess hyper(std::string(runtime), hyperapi::Telemetry::DoNotSendUsageDataToTableau,
                                "tabkit", {{"memory_limit",memory},{"log_file",""}});
    hyperapi::Connection connection(hyper.getEndpoint(),std::string(snapshot),hyperapi::CreateMode::None);
    // Reject views and foreign tables before running the admitted query. A view
    // could hide operations not visible to the client's SQL AST visitor.
    for (std::size_t i=0;i<table_count;++i) {
        const std::string check="SELECT COUNT(*) FROM information_schema.tables WHERE table_schema="+
            literal(schemas[i])+" AND table_name="+literal(tables[i])+" AND table_type='BASE TABLE'";
        auto result=connection.executeQuery(check);long long count=-1;
        for (const auto& row:result) count=row.get<long long>(0);
        result.close();
        if (count!=1) throw std::invalid_argument("Only an existing base table can be queried");
    }
    auto result=connection.executeQuery(std::string(sql));
    const auto& schema=result.getSchema();
    if(schema.getColumnCount()>256) throw std::length_error("column limit");
    BoundedBuffer buffer(max_bytes);std::ostream out(&buffer);
    out.exceptions(std::ios::badbit | std::ios::failbit);out.imbue(std::locale::classic());
    out << "{\"value_encoding\":\"hyper-text\",\"columns\":[";
    bool comma=false;
    for(const auto& column:schema.getColumns()) {
        if(comma)out<<',';comma=true;out<<"{\"name\":";json_string(out,column.getName().getUnescaped());
        out<<",\"sql_type\":";json_string(out,rendered(column.getType(),1024));out<<'}';
    }
    out<<"],\"rows\":[";std::size_t rows=0;bool truncated=false;
    for(const auto& row:result) {
        if(rows==max_rows){truncated=true;break;}
        if(rows++)out<<',';out<<'[';comma=false;
        for(const auto& value:row) {
            if(comma)out<<',';comma=true;
            if(value.isNull())out<<"null";
            else {
                // SDK text formatting retains SQL type separately. No conversion
                // through JSON floating point; SQL NULL remains JSON null.
                json_string(out,rendered(value,max_bytes));
            }
        }
        out<<']';
    }
    result.close();out<<"],\"truncated\":"<<(truncated?"true":"false")<<'}';return buffer.data;
}
}
extern "C" char* tabkit_hyper_query(const char* runtime,const char* memory,const char* snapshot,const char* sql,
                                    const char* const* schemas,const char* const* tables,std::size_t table_count,
                                    std::size_t max_rows,std::size_t max_bytes) noexcept {
    if(!runtime||!memory||!snapshot||!sql||max_rows==0||max_bytes<4096)
        return owned("{\"error\":\"HYPER_ARGUMENT\"}");
    try {return owned(run(runtime,memory,snapshot,sql,schemas,tables,table_count,max_rows,max_bytes));}
    catch(const std::invalid_argument&){return owned("{\"error\":\"HYPER_BASE_TABLE_REQUIRED\"}");}
    catch(const std::length_error&){return owned("{\"error\":\"HYPER_RESULT_LIMIT\"}");}
    catch(const std::exception&){return owned("{\"error\":\"HYPER_QUERY_FAILED\"}");}
    catch(...){return owned("{\"error\":\"HYPER_NATIVE_FAILURE\"}");}
}
extern "C" void tabkit_hyper_free(char* p) noexcept {std::free(p);}
