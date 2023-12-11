use app_config::memcache::MemcacheConnectionString;

pub struct MemcacheConnectionStringWrapper(Vec<String>);

impl From<&MemcacheConnectionString> for MemcacheConnectionStringWrapper {
    fn from(value: &MemcacheConnectionString) -> Self {
        Self(value.get_urls())
    }
}

impl memcache::Connectable for MemcacheConnectionStringWrapper {
    fn get_urls(self) -> Vec<String> {
        self.0.get_urls()
    }
}

impl r2d2_memcache::memcache::Connectable for MemcacheConnectionStringWrapper {
    fn get_urls(self) -> Vec<String> {
        self.0.get_urls()
    }
}
