// Owned bridge to the official Tableau Hyper C++ SDK. No vendor code is copied.
// Only SELECT queries reach executeQuery. The database argument is a disposable
// snapshot created by Rust; we never connect the SDK to the user's source file.
#include <hyperapi/hyperapi.hpp>
#include <algorithm>
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
#include <vector>

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
char* owned(const std::string& s) noexcept {
    char* p=static_cast<char*>(std::malloc(s.size()+1));
    if (p) std::memcpy(p,s.c_str(),s.size()+1);return p;
}
using MetadataRow = std::vector<std::string>;
std::string metadata_json(const std::vector<std::pair<std::string,std::string>>& columns,
                          const std::vector<MetadataRow>& rows,std::size_t max_rows,std::size_t max_bytes) {
    BoundedBuffer buffer(max_bytes);std::ostream out(&buffer);
    out.exceptions(std::ios::badbit | std::ios::failbit);
    out << "{\"value_encoding\":\"hyper-text\",\"columns\":[";
    for(std::size_t i=0;i<columns.size();++i) {
        if(i)out<<',';out<<"{\"name\":";json_string(out,columns[i].first);
        out<<",\"sql_type\":";json_string(out,columns[i].second);out<<'}';
    }
    out<<"],\"rows\":[";
    for(std::size_t i=0;i<std::min(rows.size(),max_rows);++i) {
        if(i)out<<',';out<<'[';
        for(std::size_t j=0;j<rows[i].size();++j) {
            if(j)out<<',';json_string(out,rows[i][j]);
        }
        out<<']';
    }
    out<<"],\"truncated\":"<<(rows.size()>max_rows?"true":"false")<<'}';
    return buffer.data;
}
bool base_table(const hyperapi::Catalog& catalog,const std::string& schema,const std::string& table) {
    for(const auto& name:catalog.getTableNames(hyperapi::SchemaName(schema)))
        if(name.getName().getUnescaped()==table)return true;
    return false;
}
std::string run(const char* runtime,const char* memory,const char* snapshot,const char* sql,
                int mode,const char* selector_schema,const char* selector_table,
                const char* const* schemas,const char* const* tables,std::size_t table_count,
                std::size_t max_rows,std::size_t max_bytes) {
    hyperapi::HyperProcess hyper(std::string(runtime), hyperapi::Telemetry::DoNotSendUsageDataToTableau,
                                "tabkit", {{"memory_limit",memory}});
    hyperapi::Connection connection(hyper.getEndpoint(),std::string(snapshot),hyperapi::CreateMode::None);
    const auto& catalog=connection.getCatalog();
    // Hyper extracts need SDK catalog calls for metadata; information_schema is absent.
    if(mode==1) {
        std::vector<MetadataRow> rows;
        for(const auto& schema:catalog.getSchemaNames()) {
            const auto& name=schema.getName().getUnescaped();
            if(name=="pg_catalog"||name=="information_schema")continue;
            for(const auto& table:catalog.getTableNames(schema))
                rows.push_back({name,table.getName().getUnescaped(),"BASE TABLE"});
        }
        std::sort(rows.begin(),rows.end());
        return metadata_json({{"table_schema","TEXT"},{"table_name","TEXT"},{"table_type","TEXT"}},rows,max_rows,max_bytes);
    }
    if(mode==2) {
        if(!base_table(catalog,selector_schema,selector_table))
            throw std::invalid_argument("Only an existing base table can be inspected");
        auto definition=catalog.getTableDefinition(hyperapi::TableName(hyperapi::SchemaName(selector_schema),hyperapi::Name(selector_table)));
        std::vector<MetadataRow> rows;
        for(const auto& column:definition.getColumns())
            rows.push_back({column.getName().getUnescaped(),rendered(column.getType(),1024),
                            column.getNullability()==hyperapi::Nullability::Nullable?"YES":"NO",
                            std::to_string(rows.size()+1)});
        return metadata_json({{"column_name","TEXT"},{"data_type","TEXT"},{"is_nullable","TEXT"},{"ordinal_position","BIGINT"}},rows,max_rows,max_bytes);
    }
    if(mode!=0)throw std::invalid_argument("Invalid operation");
    // Reject views and foreign tables before running the admitted query. A view
    // could hide operations not visible to the client's SQL AST visitor.
    for (std::size_t i=0;i<table_count;++i) {
        if(!base_table(catalog,schemas[i],tables[i]))
            throw std::invalid_argument("Only an existing base table can be queried");
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
                                    int mode,const char* selector_schema,const char* selector_table,
                                    const char* const* schemas,const char* const* tables,std::size_t table_count,
                                    std::size_t max_rows,std::size_t max_bytes) noexcept {
    if(!runtime||!memory||!snapshot||!sql||!selector_schema||!selector_table||max_rows==0||max_bytes<4096)
        return owned("{\"error\":\"HYPER_ARGUMENT\"}");
    try {return owned(run(runtime,memory,snapshot,sql,mode,selector_schema,selector_table,schemas,tables,table_count,max_rows,max_bytes));}
    catch(const std::invalid_argument&){return owned("{\"error\":\"HYPER_BASE_TABLE_REQUIRED\"}");}
    catch(const std::length_error&){return owned("{\"error\":\"HYPER_RESULT_LIMIT\"}");}
    catch(const std::exception&){return owned("{\"error\":\"HYPER_QUERY_FAILED\"}");}
    catch(...){return owned("{\"error\":\"HYPER_NATIVE_FAILURE\"}");}
}
extern "C" void tabkit_hyper_free(char* p) noexcept {std::free(p);}
